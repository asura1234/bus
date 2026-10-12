use super::*;
use crate::utils::perf::{Counter, Gauge, LoopWatch, Timer};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

fn options() -> PerfOptions {
    PerfOptions {
        enabled: true,
        window: Duration::from_secs(1),
        stall_after: Duration::from_millis(250),
        capture_after: Some(Duration::from_secs(2)),
        capture_cooldown: Duration::from_secs(600),
        max_bytes: 1024 * 1024,
        archive: crate::utils::logging::ArchivePolicy {
            max_age: Duration::from_secs(3600),
            max_total_bytes: 1024 * 1024,
        },
        retained_captures: 2,
    }
}

fn usage(cpu_ms: u64) -> ProcessUsage {
    ProcessUsage {
        cpu_time: Duration::from_millis(cpu_ms),
        rss_bytes: 50 * 1024 * 1024,
        threads: 9,
        open_fds: 19,
        load_1m: Some(24.5),
        cpus: 12,
        system_cpu_ticks: Some((cpu_ms * 3, cpu_ms * 4)),
        swap_used_bytes: Some(2 * 1024 * 1024 * 1024),
    }
}

fn calm_usage(cpu_ms: u64) -> ProcessUsage {
    ProcessUsage {
        load_1m: Some(6.0),
        ..usage(cpu_ms)
    }
}

fn no_sample() -> Option<ProcessUsage> {
    None
}

fn no_capture(_pid: u32, _path: &Path) -> bool {
    false
}

static CAPTURES: AtomicUsize = AtomicUsize::new(0);
fn counting_capture(_pid: u32, _path: &Path) -> bool {
    CAPTURES.fetch_add(1, Ordering::SeqCst);
    true
}

fn recorder(name: &str, hooks: PerfHooks) -> (Recorder, std::path::PathBuf) {
    let dir = crate::utils::test_temp::unique_temp_path(name);
    let recorder = Recorder::open(&dir, "client", &options(), hooks).expect("open recorder");
    (recorder, dir)
}

/// A live or archived perf file; the archive worker may have gzipped it.
fn read_perf_file(path: &Path) -> Option<String> {
    use std::io::Read as _;
    let bytes = std::fs::read(path).ok()?;
    if path.extension().is_some_and(|ext| ext == "gz") {
        let mut text = String::new();
        flate2::read::GzDecoder::new(&bytes[..])
            .read_to_string(&mut text)
            .ok()?;
        return Some(text);
    }
    String::from_utf8(bytes).ok()
}

fn lines(path: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).expect("every line is JSON"))
        .collect()
}

#[test]
fn options_are_on_by_default_and_env_can_disable_or_tune_them() {
    let default = PerfOptions::from_lookup(|_| None);
    assert!(default.enabled);
    assert_eq!(default.window, Duration::from_secs(1));
    assert_eq!(default.stall_after, Duration::from_millis(250));
    assert_eq!(default.capture_after, Some(Duration::from_secs(1)));

    let off = PerfOptions::from_lookup(|key| (key == "BUS_PERF").then(|| "off".to_string()));
    assert!(!off.enabled);

    let tuned = PerfOptions::from_lookup(|key| match key {
        "BUS_PERF_STALL_MS" => Some("100".to_string()),
        "BUS_PERF_CAPTURE_MS" => Some("0".to_string()),
        _ => None,
    });
    assert_eq!(tuned.stall_after, Duration::from_millis(100));
    assert_eq!(tuned.capture_after, None, "0 disables stack capture");

    let garbage = PerfOptions::from_lookup(|key| (key == "BUS_PERF_STALL_MS").then(|| "x".into()));
    assert_eq!(garbage.stall_after, Duration::from_millis(250));
}

#[test]
fn cpu_percent_is_cpu_time_over_wall_time() {
    let start = Instant::now();
    let pct = cpu_percent(
        (start, Duration::from_millis(1_000)),
        (start + Duration::from_secs(2), Duration::from_millis(2_500)),
    );
    assert!((pct - 75.0).abs() < 0.01, "{pct}");
    assert_eq!(
        cpu_percent((start, Duration::ZERO), (start, Duration::ZERO)),
        0.0
    );
}

#[test]
fn window_record_has_process_sample_and_only_active_metrics() {
    static ACTIVE: Timer = Timer::new("test.rec.active");
    static IDLE: Timer = Timer::new("test.rec.idle");
    static QUEUE: Gauge = Gauge::new("test.rec.queue");
    static BYTES: Counter = Counter::new("test.rec.bytes");
    let registry = Registry::new();
    registry.add_timer(&ACTIVE);
    registry.add_timer(&IDLE);
    registry.add_gauge(&QUEUE);
    registry.add_counter(&BYTES);
    ACTIVE.record(Duration::from_millis(120));
    QUEUE.observe(7);
    BYTES.add(2048);

    let (mut recorder, dir) = recorder(
        "perf-window",
        PerfHooks {
            sample: no_sample,
            capture_stacks: no_capture,
        },
    );
    let start = Instant::now();
    let first = recorder.window_record(start, Some(usage(1_000)), &registry);
    assert_eq!(first["kind"], "window");
    assert_eq!(first["role"], "client");
    assert_eq!(first["pid"], std::process::id());
    assert!(first["ts"].as_str().unwrap().ends_with('Z'));
    let ts_ms = first["ts_ms"].as_u64().expect("epoch millis for readers");
    assert!(ts_ms.abs_diff(crate::utils::time::now_ms()) < 60_000);
    assert_eq!(first["rss_mb"], 50.0);
    assert_eq!(first["threads"], 9);
    assert_eq!(first["fds"], 19);
    assert_eq!(first["load1"], 24.5);
    assert_eq!(first["cpus"], 12);
    assert_eq!(first["swap_used_mb"], 2048.0);
    assert!(
        first.get("cpu_pct").is_none(),
        "no CPU% without a previous sample"
    );
    assert!(
        first.get("sys_cpu_pct").is_none(),
        "no system CPU% without a previous sample"
    );
    let timer = &first["timers"]["test.rec.active"];
    assert_eq!(timer["n"], 1);
    assert_eq!(timer["max_ms"], 120.0);
    assert_eq!(timer["ge16ms"], 1);
    assert_eq!(timer["ge100ms"], 1);
    assert_eq!(timer["ge1s"], 0);
    assert!(first["timers"].get("test.rec.idle").is_none());
    assert_eq!(first["gauges"]["test.rec.queue"]["max"], 7);
    assert_eq!(first["counters"]["test.rec.bytes"], 2048);

    let second = recorder.window_record(
        start + Duration::from_secs(1),
        Some(usage(1_500)),
        &registry,
    );
    assert_eq!(second["cpu_pct"], 50.0);
    // System ticks moved 1500 busy of 2000 total since the previous sample.
    assert_eq!(second["sys_cpu_pct"], 75.0);
    assert!(
        second.get("timers").is_none(),
        "no activity, no timer section"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn records_are_appended_as_jsonl_under_the_session_perf_dir() {
    let registry = Registry::new();
    let (mut recorder, dir) = recorder(
        "perf-jsonl",
        PerfHooks {
            sample: no_sample,
            capture_stacks: no_capture,
        },
    );
    let start = Instant::now();
    recorder.write_window(start, None, &registry);
    recorder.write_window(start + Duration::from_secs(1), None, &registry);
    recorder.flush();
    let path = perf_dir(&dir).join("client.jsonl");
    let records = lines(&path);
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(|record| record["kind"] == "window"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn watchdog_writes_stall_records_and_captures_stacks_once_per_cooldown() {
    static WATCH: LoopWatch = LoopWatch::new("test.rec.loop");
    let registry = Registry::new();
    registry.add_watch(&WATCH);
    let (mut recorder, dir) = recorder(
        "perf-stall",
        PerfHooks {
            sample: no_sample,
            capture_stacks: counting_capture,
        },
    );
    let before = CAPTURES.load(Ordering::SeqCst);
    let start = Instant::now();

    let guard = WATCH.enter("present");
    let t0 = now_us();
    recorder.watch(start, t0 + 300_000, &registry);
    assert_eq!(
        CAPTURES.load(Ordering::SeqCst),
        before,
        "no capture below 2s"
    );
    recorder.watch(start, t0 + 2_100_000, &registry);
    recorder.watch(start, t0 + 2_200_000, &registry);
    recorder.wait_for_captures();
    assert_eq!(
        CAPTURES.load(Ordering::SeqCst),
        before + 1,
        "one capture per stall"
    );
    drop(guard);
    recorder.watch(start, now_us(), &registry);

    let second = WATCH.enter("present");
    let t1 = now_us();
    recorder.watch(start + Duration::from_secs(60), t1 + 2_500_000, &registry);
    recorder.wait_for_captures();
    assert_eq!(
        CAPTURES.load(Ordering::SeqCst),
        before + 1,
        "cooldown holds"
    );
    drop(second);
    recorder.watch(start + Duration::from_secs(60), now_us(), &registry);
    recorder.flush();

    let records = lines(&perf_dir(&dir).join("client.jsonl"));
    let stalls: Vec<_> = records.iter().filter(|r| r["kind"] == "stall").collect();
    assert_eq!(stalls.len(), 5, "{records:?}");
    assert_eq!(stalls[0]["state"], "began");
    assert_eq!(stalls[0]["loop"], "test.rec.loop");
    assert_eq!(stalls[0]["stage"], "present");
    assert!(stalls[0]["busy_ms"].as_u64().unwrap() >= 300);
    assert_eq!(stalls[1]["state"], "capture");
    assert_eq!(stalls[1]["stage"], "present");
    assert_eq!(stalls[1]["ok"], true);
    let capture = stalls[1]["file"].as_str().unwrap();
    assert!(
        capture.starts_with("stall-") && capture.ends_with("-client.txt"),
        "{capture}"
    );
    assert_eq!(stalls[2]["state"], "ended");
    assert_eq!(stalls[3]["state"], "began");
    assert_eq!(stalls[4]["state"], "ended");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn watchdog_lag_is_recorded_when_the_perf_thread_wakes_late() {
    let (mut recorder, dir) = recorder(
        "perf-lag",
        PerfHooks {
            sample: no_sample,
            capture_stacks: no_capture,
        },
    );
    recorder.note_wake(Duration::from_millis(100), Duration::from_millis(350));
    let registry = Registry::new();
    let record = recorder.window_record(Instant::now(), None, &registry);
    assert_eq!(record["watchdog_lag_max_ms"], 250.0);
    let next = recorder.window_record(Instant::now(), None, &registry);
    assert!(next.get("watchdog_lag_max_ms").is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn start_is_a_no_op_when_disabled() {
    let dir = crate::utils::test_temp::unique_temp_path("perf-off");
    let mut disabled = options();
    disabled.enabled = false;
    assert!(!start(
        &dir,
        "server",
        &disabled,
        PerfHooks {
            sample: no_sample,
            capture_stacks: no_capture
        }
    ));
    assert!(!perf_dir(&dir).exists());
}

#[test]
fn stall_warnings_carry_the_latest_process_and_system_sample() {
    static WATCH: LoopWatch = LoopWatch::new("test.rec.warn");
    let registry = Registry::new();
    registry.add_watch(&WATCH);
    let (mut recorder, dir) = recorder(
        "perf-warn",
        PerfHooks {
            sample: no_sample,
            capture_stacks: no_capture,
        },
    );
    let start = Instant::now();
    recorder.window_record(start, Some(usage(1_000)), &registry);
    recorder.window_record(
        start + Duration::from_secs(1),
        Some(usage(1_900)),
        &registry,
    );
    let guard = WATCH.enter("journal");
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| recorder.watch(start, now_us() + 300_000, &registry));
    drop(guard);
    let text = capture.text();
    let line = text
        .lines()
        .find(|line| line.contains("perf.stall"))
        .expect(&text);
    for field in [
        "WARN",
        "loop_name=\"test.rec.warn\"",
        "stage=\"journal\"",
        "busy_ms=",
        "cpu_pct=90",
        "rss_mb=50",
        "load1=24.5",
        "sys_cpu_pct=75",
    ] {
        assert!(line.contains(field), "{field} missing: {line}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn high_machine_load_warns_at_most_once_a_minute() {
    let registry = Registry::new();
    let (mut loaded, dir) = recorder(
        "perf-load",
        PerfHooks {
            sample: no_sample,
            capture_stacks: no_capture,
        },
    );
    let mut busy = usage(1_000);
    busy.load_1m = Some(30.0);
    let start = Instant::now();
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        loaded.window_record(start, Some(busy), &registry);
        loaded.window_record(start + Duration::from_secs(1), Some(busy), &registry);
        loaded.window_record(start + Duration::from_secs(61), Some(busy), &registry);
    });
    let text = capture.text();
    let warns: Vec<_> = text
        .lines()
        .filter(|line| line.contains("perf.high_load"))
        .collect();
    assert_eq!(warns.len(), 2, "{text}");
    assert!(
        warns[0].contains("WARN") && warns[0].contains("load1=30") && warns[0].contains("cpus=12"),
        "{}",
        warns[0]
    );

    let quiet = crate::utils::logging::test_capture::Capture::default();
    let (mut calm, calm_dir) = recorder(
        "perf-calm",
        PerfHooks {
            sample: no_sample,
            capture_stacks: no_capture,
        },
    );
    quiet.run(|| {
        calm.window_record(start, Some(calm_usage(1_000)), &registry);
        calm.window_record(
            start + Duration::from_secs(1),
            Some(calm_usage(1_100)),
            &registry,
        );
    });
    assert!(!quiet.text().contains("perf.high_load"), "{}", quiet.text());
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(calm_dir);
}

#[test]
fn stack_captures_prune_old_files_to_a_bounded_retention() {
    fn write_capture(_pid: u32, path: &Path) -> bool {
        std::fs::write(path, "stack frames\n").is_ok()
    }
    let (mut recorder, dir) = recorder(
        "perf-capture-retention",
        PerfHooks {
            sample: no_sample,
            capture_stacks: write_capture,
        },
    );
    for day in 1..=6 {
        std::fs::write(
            perf_dir(&dir).join(format!("stall-202601{day:02}T000000Z-client.txt")),
            "old stack frames\n",
        )
        .unwrap();
    }
    recorder.capture("test.retention.loop", "present", 3_000);
    recorder.wait_for_captures();
    let captures = std::fs::read_dir(perf_dir(&dir))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("stall-"))
        .count();
    let retained = recorder.options.retained_captures;
    let _ = std::fs::remove_dir_all(dir);
    assert!(
        captures <= retained,
        "{captures} stack files remain despite {retained} retained captures"
    );
}

#[test]
fn two_client_processes_sharing_the_role_file_keep_each_others_latest_records() {
    let dir = crate::utils::test_temp::unique_temp_path("perf-shared-role");
    let small = PerfOptions {
        max_bytes: 400,
        ..options()
    };
    let hooks = PerfHooks {
        sample: no_sample,
        capture_stacks: no_capture,
    };
    let mut first = Recorder::open(&dir, "client", &small, hooks).expect("first client");
    let mut second = Recorder::open(&dir, "client", &small, hooks).expect("second client");
    second.append(&serde_json::json!({"kind": "probe", "who": "second", "n": 0}));
    for n in 0..40 {
        first.append(&serde_json::json!({"kind": "probe", "who": "first", "n": n}));
    }
    second.append(&serde_json::json!({"kind": "probe", "who": "second", "n": 1}));
    first.flush();
    second.flush();

    let live = perf_dir(&dir).join("client.jsonl");
    let found = std::iter::once(live.clone())
        .chain(crate::utils::logging::archived_files(&live))
        .filter_map(|path| read_perf_file(&path))
        .flat_map(|text| text.lines().map(str::to_owned).collect::<Vec<_>>())
        .any(|line| line.contains(r#""n":1"#) && line.contains(r#""who":"second""#));
    let _ = std::fs::remove_dir_all(dir);
    assert!(
        found,
        "the second client's latest record is in no perf file"
    );
}

#[test]
fn a_stall_that_moves_on_and_stays_blocked_in_a_later_stage_names_that_stage() {
    static WATCH: LoopWatch = LoopWatch::new("test.rec.moved");
    let registry = Registry::new();
    registry.add_watch(&WATCH);
    let (mut recorder, dir) = recorder(
        "perf-stall-moved",
        PerfHooks {
            sample: no_sample,
            capture_stacks: no_capture,
        },
    );
    let start = Instant::now();
    let guard = WATCH.enter("compose");
    let t0 = now_us();
    // Compose is a little slow and crosses the stall threshold...
    recorder.watch(start, t0 + 300_000, &registry);
    // ...then present blocks for the rest of the freeze.
    guard.stage("present");
    recorder.watch(start, t0 + 2_100_000, &registry);
    recorder.watch(start, t0 + 60_000_000, &registry);
    drop(guard);
    recorder.watch(start, now_us(), &registry);
    recorder.flush();

    let records = lines(&perf_dir(&dir).join("client.jsonl"));
    let _ = std::fs::remove_dir_all(dir);
    assert!(
        records
            .iter()
            .filter(|record| record["kind"] == "stall")
            .any(|record| record["stage"] == "present"),
        "no stall record names the stage the loop stayed blocked in: {records:?}"
    );
}

fn archive_file(dir: &Path) -> crate::utils::logging::RotatingFileState {
    std::fs::create_dir_all(perf_dir(dir)).unwrap();
    crate::utils::logging::RotatingFileState::open(
        perf_dir(dir).join("client.jsonl"),
        1024 * 1024,
        crate::utils::logging::Retention::Archive(options().archive),
    )
    .unwrap()
}

#[cfg(unix)]
#[test]
fn append_failures_warn_once_and_recovery_reports_the_lost_records() {
    use crate::utils::perf::writer::PerfWriter;
    let dir = crate::utils::test_temp::unique_temp_path("perf-append-failure");
    let dropped = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let mut writer = PerfWriter::new("client", archive_file(&dir), dropped.clone());
    let live = perf_dir(&dir).join("client.jsonl");
    // A directory where the file was makes every reopen fail.
    std::fs::remove_file(&live).unwrap();
    std::fs::create_dir(&live).unwrap();
    let failing = crate::utils::logging::test_capture::Capture::default();
    failing.run(|| {
        for n in 0..3 {
            writer.write(format!("{{\"kind\":\"probe\",\"n\":{n}}}\n").as_bytes());
        }
    });
    std::fs::remove_dir(&live).unwrap();
    // Two more were refused by a full queue meanwhile.
    dropped.store(2, Ordering::SeqCst);
    let recovered = crate::utils::logging::test_capture::Capture::default();
    recovered.run(|| writer.write(b"{\"kind\":\"probe\",\"n\":3}\n"));

    let records = lines(&live);
    let _ = std::fs::remove_dir_all(dir);
    let failed = failing.text();
    assert_eq!(failed.matches("perf.write_failed").count(), 1, "{failed}");
    assert!(
        failed.contains("role=\"client\"") || failed.contains("role=client"),
        "{failed}"
    );
    let recovered = recovered.text();
    assert!(recovered.contains("perf.write_recovered"), "{recovered}");
    assert!(recovered.contains("lost_records=5"), "{recovered}");
    assert_eq!(dropped.load(Ordering::SeqCst), 0, "the drops were reported");
    assert_eq!(records.len(), 1, "{records:?}");
}

#[cfg(unix)]
#[test]
fn a_full_writer_queue_drops_records_without_waiting_and_warns_once() {
    use crate::utils::perf::writer::WriterHandle;
    let dir = crate::utils::test_temp::unique_temp_path("perf-full-queue");
    let handle = WriterHandle::spawn("client", archive_file(&dir), 1).unwrap();
    let live = perf_dir(&dir).join("client.jsonl");
    let holder = std::fs::OpenOptions::new()
        .append(true)
        .open(&live)
        .unwrap();
    holder.lock().unwrap();

    let capture = crate::utils::logging::test_capture::Capture::default();
    let started = Instant::now();
    capture.run(|| {
        for n in 0..6 {
            handle.send(format!("{{\"kind\":\"probe\",\"n\":{n}}}\n").into_bytes());
        }
    });
    let elapsed = started.elapsed();
    holder.unlock().unwrap();
    handle.flush();

    let records = lines(&live);
    let _ = std::fs::remove_dir_all(dir);
    assert!(
        elapsed < Duration::from_millis(100),
        "queuing waited {elapsed:?}"
    );
    let text = capture.text();
    assert_eq!(text.matches("perf.write_failed").count(), 1, "{text}");
    assert!(
        (1..6).contains(&records.len()),
        "the writer held at most the queue plus one in flight: {records:?}"
    );
}

#[cfg(unix)]
#[test]
fn perf_window_does_not_block_the_watchdog_behind_another_writer() {
    let (mut recorder, dir) = recorder(
        "perf-contended-writer",
        PerfHooks {
            sample: no_sample,
            capture_stacks: no_capture,
        },
    );
    let live = perf_dir(&dir).join("client.jsonl");
    let (locked_tx, locked_rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(live)
            .unwrap();
        file.lock().unwrap();
        locked_tx.send(()).unwrap();
        // A descheduled client or a client waiting on filesystem I/O can hold
        // this shared role-file lock while another client's watchdog wakes.
        std::thread::sleep(Duration::from_millis(300));
        file.unlock().unwrap();
    });
    locked_rx.recv().unwrap();
    let started = Instant::now();
    recorder.write_window(started, None, &Registry::new());
    let elapsed = started.elapsed();
    holder.join().unwrap();
    let _ = std::fs::remove_dir_all(dir);
    assert!(
        elapsed < Duration::from_millis(100),
        "watchdog window append waited for another writer for {elapsed:?}"
    );
}
