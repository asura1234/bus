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
        capture_retention: ArchivePolicy {
            max_age: Duration::from_secs(3600),
            max_total_bytes: 32,
        },
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
        thread_cpu: None,
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
fn counting_capture(_pid: u32, path: &Path) -> bool {
    CAPTURES.fetch_add(1, Ordering::SeqCst);
    std::fs::write(path, "stacks").is_ok()
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
fn windows_report_each_threads_cpu_delta_and_reset_missing_threads() {
    let registry = Registry::new();
    let (mut recorder, dir) = recorder(
        "perf-thread-deltas",
        PerfHooks {
            sample: no_sample,
            machine: || None,
            capture_stacks: no_capture,
        },
    );
    let sampled = |ui_ms, coordinator_ms| {
        let mut usage = usage(ui_ms + coordinator_ms);
        usage.thread_cpu = Some(vec![
            ThreadUsage {
                id: 11,
                name: "ui".into(),
                cpu_time: Duration::from_millis(ui_ms),
            },
            ThreadUsage {
                id: 22,
                name: "coordinator".into(),
                cpu_time: Duration::from_millis(coordinator_ms),
            },
        ]);
        usage
    };
    let start = Instant::now();
    let first = recorder.window_record(start, Some(sampled(100, 1000)), &registry);
    assert!(first["thread_cpu"][0].get("cpu_pct").is_none());
    let second = recorder.window_record(
        start + Duration::from_secs(2),
        Some(sampled(600, 2500)),
        &registry,
    );
    assert_eq!(second["thread_cpu"][0]["id"], 11);
    assert_eq!(second["thread_cpu"][0]["name"], "ui");
    assert_eq!(second["thread_cpu"][0]["cpu_pct"], 25.0);
    assert_eq!(second["thread_cpu"][1]["cpu_pct"], 75.0);
    recorder.window_record(start + Duration::from_secs(3), Some(usage(4000)), &registry);
    let returned = recorder.window_record(
        start + Duration::from_secs(4),
        Some(sampled(700, 2600)),
        &registry,
    );
    assert!(
        returned["thread_cpu"][0].get("cpu_pct").is_none(),
        "missing sample breaks the baseline"
    );
    let _ = std::fs::remove_dir_all(dir);
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
            machine: || None,
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
            machine: || None,
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
            machine: || None,
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
    assert_eq!(
        stalls
            .iter()
            .map(|r| r["state"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "began",
            "capture_requested",
            "capture",
            "ended",
            "began",
            "capture_suppressed",
            "ended"
        ],
        "{records:?}"
    );
    assert_eq!(stalls[0]["state"], "began");
    assert_eq!(stalls[0]["loop"], "test.rec.loop");
    assert_eq!(stalls[0]["stage"], "present");
    assert!(stalls[0]["busy_ms"].as_u64().unwrap() >= 300);
    assert_eq!(stalls[2]["state"], "capture");
    assert_eq!(stalls[2]["stage"], "present");
    assert_eq!(stalls[2]["ok"], true);
    let capture = stalls[2]["file"].as_str().unwrap();
    assert!(
        capture.starts_with("stall-") && capture.ends_with("-client.txt"),
        "{capture}"
    );
    assert_eq!(stalls[3]["state"], "ended");
    assert_eq!(stalls[4]["state"], "began");
    assert_eq!(stalls[6]["state"], "ended");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn watchdog_lag_is_recorded_when_the_perf_thread_wakes_late() {
    let (mut recorder, dir) = recorder(
        "perf-lag",
        PerfHooks {
            sample: no_sample,
            machine: || None,
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
fn coordinator_capture_does_not_suppress_later_ui_capture() {
    static COORDINATOR: LoopWatch = LoopWatch::new("test.independent.coordinator");
    static UI: LoopWatch = LoopWatch::new("test.independent.ui");
    let registry = Registry::new();
    registry.add_watch(&COORDINATOR);
    registry.add_watch(&UI);
    let (mut recorder, dir) = recorder(
        "perf-independent-captures",
        PerfHooks {
            sample: no_sample,
            machine: || None,
            capture_stacks: no_capture,
        },
    );
    let start = Instant::now();
    let coordinator = COORDINATOR.enter("save");
    recorder.watch(start, now_us() + 2_100_000, &registry);
    recorder.wait_for_captures();
    drop(coordinator);
    recorder.watch(start, now_us(), &registry);
    let ui = UI.enter("compose");
    recorder.watch(
        start + Duration::from_secs(60),
        now_us() + 2_100_000,
        &registry,
    );
    recorder.wait_for_captures();
    recorder.watch(
        start + Duration::from_secs(61),
        now_us() + 2_200_000,
        &registry,
    );
    recorder.wait_for_captures();
    drop(ui);
    recorder.flush();
    let records = lines(&perf_dir(&dir).join("client.jsonl"));
    let captures: Vec<_> = records.iter().filter(|r| r["state"] == "capture").collect();
    assert_eq!(
        captures.len(),
        2,
        "each loop gets its own cooldown: {records:?}"
    );
    assert_eq!(captures[0]["loop"], "test.independent.coordinator");
    assert_eq!(captures[1]["loop"], "test.independent.ui");
    assert_ne!(
        captures[0]["file"], captures[1]["file"],
        "captures must not overwrite each other"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn start_is_a_no_op_when_disabled() {
    let dir = crate::utils::test_temp::unique_temp_path("perf-off");
    let mut disabled = options();
    disabled.enabled = false;
    assert!(start(
        &dir,
        "server",
        &disabled,
        PerfHooks {
            sample: no_sample,
            machine: || None,
            capture_stacks: no_capture
        }
    )
    .is_none());
    assert!(!perf_dir(&dir).exists());
}

#[test]
fn concurrent_loop_captures_keep_retention_bounded() {
    static READY: std::sync::Barrier = std::sync::Barrier::new(2);
    static RELEASE: std::sync::Barrier = std::sync::Barrier::new(2);
    fn delayed_capture(_pid: u32, path: &Path) -> bool {
        READY.wait();
        RELEASE.wait();
        std::fs::write(path, "stacks").is_ok()
    }
    let (mut recorder, dir) = recorder(
        "perf-one-capture-per-pid",
        PerfHooks {
            sample: no_sample,
            machine: || None,
            capture_stacks: delayed_capture,
        },
    );
    recorder.capture("coordinator", "save", 2000);
    READY.wait();
    recorder.capture("client.ui_loop", "compose", 2000);
    assert_eq!(recorder.pending_captures.len(), 1);
    RELEASE.wait();
    recorder.wait_for_captures();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn pruning_does_not_delete_an_active_capture_output() {
    static READY: std::sync::Barrier = std::sync::Barrier::new(2);
    static RELEASE: std::sync::Barrier = std::sync::Barrier::new(2);
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    fn capture(_pid: u32, path: &Path) -> bool {
        let first = CALLS.fetch_add(1, Ordering::SeqCst) == 0;
        let ok = std::fs::write(path, "stacks").is_ok();
        if first {
            READY.wait();
            RELEASE.wait();
        }
        ok
    }
    let (mut recorder, dir) = recorder(
        "perf-active-output",
        PerfHooks {
            sample: no_sample,
            machine: || None,
            capture_stacks: capture,
        },
    );
    recorder.options.capture_retention.max_total_bytes = 6;
    recorder.capture("coordinator", "save", 2000);
    READY.wait();
    let active = std::fs::read_dir(perf_dir(&dir))
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == "part"))
        .unwrap();
    recorder.capture("ui", "compose", 2000);
    let preserved = active.is_file();
    RELEASE.wait();
    recorder.wait_for_captures();
    let _ = std::fs::remove_dir_all(dir);
    assert!(
        preserved,
        "a later capture pruned the output still being written"
    );
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
            machine: || None,
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
            machine: || None,
            capture_stacks: no_capture,
        },
    );
    let mut busy = usage(1_000);
    busy.load_1m = Some(30.0);
    let start = Instant::now();
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        loaded.window_record(start, Some(busy.clone()), &registry);
        loaded.window_record(
            start + Duration::from_secs(1),
            Some(busy.clone()),
            &registry,
        );
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
            machine: || None,
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
            machine: || None,
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
    let retained = (recorder.options.capture_retention.max_total_bytes / 6) as usize;
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
        machine: || None,
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
            machine: || None,
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

/// Spawns a writer whose thread logs into `capture`.
fn captured_writer(
    capture: &crate::utils::logging::test_capture::Capture,
    dir: &Path,
    capacity: usize,
) -> crate::utils::perf::writer::WriterHandle {
    let mut handle = None;
    capture.run(|| {
        handle = Some(
            crate::utils::perf::writer::WriterHandle::spawn("client", archive_file(dir), capacity)
                .unwrap(),
        );
    });
    handle.unwrap()
}

#[cfg(unix)]
#[test]
fn append_failures_warn_once_and_the_episode_reports_the_lost_records_once_quiet() {
    use crate::utils::perf::writer::{Counters, PerfWriter, QUIET_AFTER_LOSS};
    let dir = crate::utils::test_temp::unique_temp_path("perf-append-failure");
    let counters = std::sync::Arc::new(Counters::default());
    let mut writer = PerfWriter::new("client", archive_file(&dir), counters.clone());
    let live = perf_dir(&dir).join("client.jsonl");
    // A directory where the file was makes every reopen fail.
    std::fs::remove_file(&live).unwrap();
    std::fs::create_dir(&live).unwrap();
    let t0 = Instant::now();
    let capture = crate::utils::logging::test_capture::Capture::default();
    let mut texts = Vec::new();
    capture.run(|| {
        for n in 0..3 {
            writer.write(format!("{{\"kind\":\"probe\",\"n\":{n}}}\n").as_bytes(), t0);
        }
    });
    texts.push(capture.text());
    std::fs::remove_dir(&live).unwrap();
    // Two more were refused by a full queue meanwhile.
    counters.dropped.store(2, Ordering::SeqCst);
    capture.run(|| {
        writer.write(b"{\"kind\":\"probe\",\"n\":3}\n", t0);
        writer.account(t0);
    });
    texts.push(capture.text());
    capture.run(|| writer.account(t0 + QUIET_AFTER_LOSS));
    texts.push(capture.text());

    let records = lines(&live);
    let _ = std::fs::remove_dir_all(dir);
    let [failing, still_losing, quiet] = [&texts[0], &texts[1], &texts[2]];
    assert_eq!(failing.matches("perf.write_failed").count(), 1, "{failing}");
    assert!(
        failing.contains("role=\"client\"") || failing.contains("role=client"),
        "{failing}"
    );
    assert_eq!(
        still_losing.matches("perf.").count(),
        1,
        "the episode continues silently until quiet: {still_losing}"
    );
    assert!(quiet.contains("perf.write_recovered"), "{quiet}");
    assert!(quiet.contains("lost_records=5"), "{quiet}");
    assert_eq!(
        counters.dropped.load(Ordering::SeqCst),
        0,
        "the drops were taken"
    );
    assert_eq!(records.len(), 1, "{records:?}");
}

#[cfg(unix)]
#[test]
fn a_full_writer_queue_drops_records_without_waiting_and_the_writer_warns_once() {
    let dir = crate::utils::test_temp::unique_temp_path("perf-full-queue");
    let writer_log = crate::utils::logging::test_capture::Capture::default();
    let handle = captured_writer(&writer_log, &dir, 1);
    let live = perf_dir(&dir).join("client.jsonl");
    let holder = std::fs::OpenOptions::new()
        .append(true)
        .open(&live)
        .unwrap();
    holder.lock().unwrap();

    let watchdog_log = crate::utils::logging::test_capture::Capture::default();
    let started = Instant::now();
    watchdog_log.run(|| {
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
    assert_eq!(
        watchdog_log.text(),
        "",
        "the watchdog never logs a loss itself"
    );
    let text = writer_log.text();
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
            machine: || None,
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

#[cfg(unix)]
#[test]
fn normal_process_exit_preserves_records_queued_behind_a_busy_writer() {
    const CHILD_DIR: &str = "BUS_PERF_EXIT_PROBE_DIR";
    if let Some(dir) = std::env::var_os(CHILD_DIR) {
        let mut recorder = Recorder::open(
            Path::new(&dir),
            "client",
            &options(),
            PerfHooks {
                sample: no_sample,
                machine: || None,
                capture_stacks: no_capture,
            },
        )
        .unwrap();
        for n in 0..16 {
            recorder.append(&serde_json::json!({"kind":"probe", "n":n}));
        }
        // Return normally. Production also exits without the test-only flush.
        return;
    }
    let dir = crate::utils::test_temp::unique_temp_path("perf-exit-backlog");
    std::fs::create_dir_all(perf_dir(&dir)).unwrap();
    let live = perf_dir(&dir).join("client.jsonl");
    let holder = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&live)
        .unwrap();
    holder.lock().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("utils::perf::recorder::recorder_test::normal_process_exit_preserves_records_queued_behind_a_busy_writer")
        .env(CHILD_DIR, &dir)
        .spawn().unwrap();
    // The contention is bounded: release it even if shutdown chooses to wait.
    std::thread::sleep(Duration::from_millis(300));
    holder.unlock().unwrap();
    let status = child.wait().unwrap();
    let records = lines(&live);
    let _ = std::fs::remove_dir_all(dir);
    assert!(status.success(), "child must exit normally: {status}");
    assert_eq!(
        records.len(),
        16,
        "normal exit must drain accepted perf records"
    );
}

#[cfg(unix)]
#[test]
fn full_perf_queue_warning_does_not_block_watchdog_on_the_log_file() {
    use crate::utils::perf::writer::WriterHandle;
    let dir = crate::utils::test_temp::unique_temp_path("perf-overflow-log-lock");
    let handle = WriterHandle::spawn("client", archive_file(&dir), 1).unwrap();
    let perf_lock = std::fs::OpenOptions::new()
        .append(true)
        .open(perf_dir(&dir).join("client.jsonl"))
        .unwrap();
    perf_lock.lock().unwrap();
    let log_path = dir.join("client.log");
    let (locked_tx, locked_rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
            .unwrap();
        file.lock().unwrap();
        locked_tx.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        file.unlock().unwrap();
    });
    locked_rx.recv().unwrap();
    let logging = crate::utils::logging::LoggingOptions {
        filter: tracing_subscriber::EnvFilter::new("warn"),
        max_bytes: 1024 * 1024,
        retention: crate::utils::logging::Retention::Archive(options().archive),
        dev: true,
    };
    let mut elapsed = Duration::ZERO;
    crate::utils::logging::test_capture::run_to_file(dir.clone(), "client.log", &logging, || {
        let started = Instant::now();
        for n in 0..6 {
            handle.send(format!("{{\"kind\":\"probe\",\"n\":{n}}}\n").into_bytes());
        }
        elapsed = started.elapsed();
    });
    holder.join().unwrap();
    perf_lock.unlock().unwrap();
    handle.flush();
    let _ = std::fs::remove_dir_all(dir);
    assert!(
        elapsed < Duration::from_millis(100),
        "queue overflow logging blocked the watchdog for {elapsed:?}"
    );
}

#[cfg(unix)]
#[test]
fn a_writer_that_stays_slower_than_the_watchdog_warns_once_per_overload_not_per_write() {
    let dir = crate::utils::test_temp::unique_temp_path("perf-sustained-overload");
    // The writer reports losses; its thread logs into this capture.
    let capture = crate::utils::logging::test_capture::Capture::default();
    let handle = captured_writer(&capture, &dir, 1);
    let live = perf_dir(&dir).join("client.jsonl");
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let holder = {
        let stop = std::sync::Arc::clone(&stop);
        let live = live.clone();
        std::thread::spawn(move || {
            let file = std::fs::OpenOptions::new().append(true).open(live).unwrap();
            // Every append waits ~100 ms on the shared lock: a slow disk or a
            // busy peer keeps the writer slower than the records arriving.
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                file.lock().unwrap();
                std::thread::sleep(Duration::from_millis(100));
                file.unlock().unwrap();
                std::thread::sleep(Duration::from_millis(1));
            }
        })
    };
    {
        let until = Instant::now() + Duration::from_secs(2);
        let mut n = 0;
        while Instant::now() < until {
            handle.send(format!("{{\"kind\":\"probe\",\"n\":{n}}}\n").into_bytes());
            n += 1;
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    holder.join().unwrap();
    handle.flush();
    let _ = std::fs::remove_dir_all(dir);
    let text = capture.text();
    let warns =
        text.matches("perf.write_failed").count() + text.matches("perf.write_recovered").count();
    assert!(
        text.contains("perf.write_failed"),
        "the overload is reported by the writer: {text}"
    );
    assert!(
        warns <= 4,
        "{warns} loss WARNs in 2 s of one continuous overload: {text}"
    );
}

#[cfg(unix)]
#[test]
fn exit_drain_gives_up_at_the_deadline_and_reports_the_undrained_records() {
    let dir = crate::utils::test_temp::unique_temp_path("perf-drain-deadline");
    let writer_log = crate::utils::logging::test_capture::Capture::default();
    let mut handle = captured_writer(&writer_log, &dir, 16);
    let live = perf_dir(&dir).join("client.jsonl");
    let holder = std::fs::OpenOptions::new()
        .append(true)
        .open(&live)
        .unwrap();
    holder.lock().unwrap();
    for n in 0..5 {
        handle.send(format!("{{\"kind\":\"probe\",\"n\":{n}}}\n").into_bytes());
    }
    let exit_log = crate::utils::logging::test_capture::Capture::default();
    let started = Instant::now();
    exit_log.run(|| handle.shutdown(Duration::from_millis(200)));
    let elapsed = started.elapsed();
    // The loss is reported off the exiting thread.
    let reported = Instant::now() + Duration::from_secs(2);
    while !exit_log.text().contains("undrained_records") && Instant::now() < reported {
        std::thread::sleep(Duration::from_millis(5));
    }
    holder.unlock().unwrap();
    drop(handle);
    let _ = std::fs::remove_dir_all(dir);
    assert!(
        elapsed < Duration::from_millis(500),
        "exit waited {elapsed:?} on a writer that cannot write"
    );
    let text = exit_log.text();
    assert!(text.contains("perf.write_failed"), "{text}");
    // One record is in flight on the blocked writer; four wait behind it.
    assert!(
        text.contains("undrained_records=5") || text.contains("undrained_records=4"),
        "{text}"
    );
}

#[test]
fn dropping_the_running_recorder_stops_it_promptly() {
    if isolated_lifecycle_test("dropping_the_running_recorder_stops_it_promptly") {
        return;
    }
    let dir = crate::utils::test_temp::unique_temp_path("perf-stop");
    let running = start(
        &dir,
        "client",
        &options(),
        PerfHooks {
            sample: no_sample,
            machine: || None,
            capture_stacks: no_capture,
        },
    )
    .expect("recorder starts");
    let started = Instant::now();
    drop(running);
    let elapsed = started.elapsed();
    let _ = std::fs::remove_dir_all(dir);
    assert!(
        elapsed < Duration::from_secs(1),
        "stopping took {elapsed:?}"
    );
}

#[cfg(unix)]
#[test]
fn shutdown_deadline_does_not_wait_for_a_locked_diagnostic_log() {
    let dir = crate::utils::test_temp::unique_temp_path("perf-exit-log-deadline");
    let capture = crate::utils::logging::test_capture::Capture::default();
    let mut handle = captured_writer(&capture, &dir, 16);
    let perf_lock = std::fs::OpenOptions::new()
        .append(true)
        .open(perf_dir(&dir).join("client.jsonl"))
        .unwrap();
    perf_lock.lock().unwrap();
    handle.send(b"{\"kind\":\"probe\"}\n".to_vec());
    let log_path = dir.join("client.log");
    let (locked_tx, locked_rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
            .unwrap();
        file.lock().unwrap();
        locked_tx.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        file.unlock().unwrap();
    });
    locked_rx.recv().unwrap();
    let logging = crate::utils::logging::LoggingOptions {
        filter: tracing_subscriber::EnvFilter::new("warn"),
        max_bytes: 1024 * 1024,
        retention: crate::utils::logging::Retention::Archive(options().archive),
        dev: true,
    };
    let mut elapsed = Duration::ZERO;
    crate::utils::logging::test_capture::run_to_file(dir.clone(), "client.log", &logging, || {
        let started = Instant::now();
        handle.shutdown(Duration::from_millis(100));
        elapsed = started.elapsed();
    });
    holder.join().unwrap();
    perf_lock.unlock().unwrap();
    drop(handle);
    let _ = std::fs::remove_dir_all(dir);
    assert!(
        elapsed < Duration::from_millis(300),
        "100 ms shutdown deadline was extended by diagnostic logging to {elapsed:?}"
    );
}

#[cfg(unix)]
#[test]
fn dropping_recorder_keeps_exit_deadline_when_watchdog_is_logging_a_stall() {
    if isolated_lifecycle_test(
        "dropping_recorder_keeps_exit_deadline_when_watchdog_is_logging_a_stall",
    ) {
        return;
    }
    static WATCH: LoopWatch = LoopWatch::new("test.rec.exit.blocked_log");
    let dir = crate::utils::test_temp::unique_temp_path("perf-stop-log-lock");
    std::fs::create_dir_all(&dir).unwrap();
    let logging = crate::utils::logging::LoggingOptions {
        filter: tracing_subscriber::EnvFilter::new("warn"),
        max_bytes: 1024 * 1024,
        retention: crate::utils::logging::Retention::Archive(options().archive),
        dev: true,
    };
    crate::utils::logging::init_file_logging_at(dir.clone(), "client.log", &logging);
    let log_path = dir.join("client.log");
    let (locked_tx, locked_rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let file = std::fs::OpenOptions::new()
            .append(true)
            .open(log_path)
            .unwrap();
        file.lock().unwrap();
        locked_tx.send(()).unwrap();
        std::thread::sleep(Duration::from_secs(3));
        file.unlock().unwrap();
    });
    locked_rx.recv().unwrap();
    let busy = WATCH.enter("present");
    let running = start(
        &dir,
        "client",
        &options(),
        PerfHooks {
            sample: no_sample,
            machine: || None,
            capture_stacks: no_capture,
        },
    )
    .unwrap();
    let wait_until = Instant::now() + Duration::from_secs(1);
    while WATCH.reported.load(Ordering::Acquire) == 0 && Instant::now() < wait_until {
        std::thread::sleep(Duration::from_millis(1));
    }
    let observed = WATCH.reported.load(Ordering::Acquire) != 0;
    let started = Instant::now();
    drop(running);
    let elapsed = started.elapsed();
    drop(busy);
    holder.join().unwrap();
    let _ = std::fs::remove_dir_all(dir);
    assert!(observed, "watchdog must have detected the blocked loop");
    assert!(
        elapsed < Duration::from_millis(2300),
        "recorder exit exceeded the 2 s drain budget while joining watchdog: {elapsed:?}"
    );
}

// Lifecycle tests need a fresh process: start is once-only, and logging plus
// the global watch registry outlive individual libtest cases.
fn isolated_lifecycle_test(name: &str) -> bool {
    const CHILD: &str = "BUS_PERF_LIFECYCLE_CHILD";
    if std::env::var(CHILD).as_deref() == Ok(name) {
        return false;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg(format!("utils::perf::recorder::recorder_test::{name}"))
        .arg("--nocapture")
        .env(CHILD, name)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("running 1 test"), "{stdout}");
    assert!(
        output.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

#[cfg(unix)]
#[test]
fn client_error_process_exit_preserves_queued_recorder_windows() {
    const CHILD_DIR: &str = "BUS_PERF_ERROR_EXIT_PROBE_DIR";
    static SAMPLES: AtomicUsize = AtomicUsize::new(0);
    fn counted_sample() -> Option<ProcessUsage> {
        SAMPLES.fetch_add(1, Ordering::SeqCst);
        None
    }
    if let Some(dir) = std::env::var_os(CHILD_DIR) {
        let mut tuned = options();
        tuned.window = Duration::from_millis(1);
        let _running = start(
            Path::new(&dir),
            "client",
            &tuned,
            PerfHooks {
                sample: counted_sample,
                machine: || None,
                capture_stacks: no_capture,
            },
        )
        .unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        while SAMPLES.load(Ordering::SeqCst) < 2 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(1));
        }
        // The single watchdog queues its first window before invoking the
        // sampler again. Two samples prove there was accepted writer work.
        assert!(
            SAMPLES.load(Ordering::SeqCst) >= 2,
            "two windows must be sampled before error exit"
        );
        std::fs::write(Path::new(&dir).join("error-exit-ready"), b"queued").unwrap();
        // finish_client_run and the server's startup failures exit this way
        // while their caller still owns the running recorder guard.
        crate::utils::perf::exit(1);
    }
    let dir = crate::utils::test_temp::unique_temp_path("perf-error-exit-backlog");
    std::fs::create_dir_all(perf_dir(&dir)).unwrap();
    let live = perf_dir(&dir).join("client.jsonl");
    let holder = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&live)
        .unwrap();
    holder.lock().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("utils::perf::recorder::recorder_test::client_error_process_exit_preserves_queued_recorder_windows")
        .env(CHILD_DIR, &dir).spawn().unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    let ready = dir.join("error-exit-ready");
    while !ready.exists() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(1));
    }
    let sampled = ready.exists();
    std::thread::sleep(Duration::from_millis(200));
    holder.unlock().unwrap();
    let status = child.wait().unwrap();
    let records = lines(&live);
    let _ = std::fs::remove_dir_all(dir);
    assert!(
        sampled,
        "child must queue a sampled window while the writer remains locked"
    );
    assert_eq!(status.code(), Some(1), "expected the client error exit");
    assert!(
        !records.is_empty(),
        "ordinary client error exit must preserve queued windows"
    );
}

#[test]
fn severe_stall_escalates_at_five_fifteen_thirty_seconds_with_one_identity() {
    static WATCH: LoopWatch = LoopWatch::new("test.freeze.escalation");
    let registry = Registry::new();
    registry.add_watch(&WATCH);
    let (mut recorder, dir) = recorder(
        "freeze-escalation",
        PerfHooks {
            sample: no_sample,
            machine: || None,
            capture_stacks: no_capture,
        },
    );
    let guard = WATCH.enter("dispatch");
    let started = WATCH.busy_since.load(Ordering::Acquire);
    let now = Instant::now();
    for seconds in [2, 5, 15, 30] {
        recorder.watch(
            now + Duration::from_secs(seconds),
            started + seconds * 1_000_000,
            &registry,
        );
        recorder.wait_for_captures();
    }
    drop(guard);
    recorder.watch(now + Duration::from_secs(31), now_us(), &registry);
    recorder.flush();
    let records = lines(&perf_dir(&dir).join("client.jsonl"));
    let captures: Vec<_> = records.iter().filter(|r| r["state"] == "capture").collect();
    assert_eq!(
        captures
            .iter()
            .map(|r| r["threshold_ms"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [2000, 5000, 15000, 30000]
    );
    assert!(captures.iter().all(|r| r["stall_id"] == started
        && r["started_us"].as_u64() <= r["ended_us"].as_u64()
        && r["started_at"].as_str().is_some()
        && r["ended_at"].as_str().is_some()));
    let stalls: Vec<_> = records
        .iter()
        .filter(|r| r["state"] == "began" || r["state"] == "ended")
        .collect();
    assert_eq!(stalls.len(), 2);
    assert_eq!(stalls[0]["stall_id"], stalls[1]["stall_id"]);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_new_severe_stall_cannot_bypass_the_previous_stalls_capture_cooldown() {
    static WATCH: LoopWatch = LoopWatch::new("test.freeze.new-episode-cooldown");
    let registry = Registry::new();
    registry.add_watch(&WATCH);
    let (mut recorder, dir) = recorder(
        "freeze-new-episode-cooldown",
        PerfHooks {
            sample: no_sample,
            machine: || None,
            capture_stacks: no_capture,
        },
    );
    let now = Instant::now();
    let first = WATCH.enter("dispatch");
    recorder.watch(now, now_us() + 2_100_000, &registry);
    recorder.wait_for_captures();
    drop(first);
    recorder.watch(now, now_us(), &registry);

    // Under load the watchdog can first observe a new episode after five
    // seconds. That is a new capture, not escalation of the previous episode.
    let second = WATCH.enter("dispatch");
    let started = WATCH.busy_since.load(Ordering::Acquire);
    for seconds in [5, 15, 30] {
        recorder.watch(
            now + Duration::from_secs(60 + seconds),
            started + seconds * 1_000_000,
            &registry,
        );
        recorder.wait_for_captures();
    }
    drop(second);
    recorder.watch(now + Duration::from_secs(91), now_us(), &registry);
    recorder.flush();
    let records = lines(&perf_dir(&dir).join("client.jsonl"));
    assert_eq!(
        records.iter().filter(|r| r["state"] == "capture").count(),
        1,
        "a new stall must respect the cooldown even at escalation thresholds: {records:?}"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dump_evictions_have_a_byte_count_and_reason_record() {
    let (mut recorder, dir) = recorder(
        "freeze-eviction-record",
        PerfHooks {
            sample: no_sample,
            machine: || None,
            capture_stacks: no_capture,
        },
    );
    std::fs::write(perf_dir(&dir).join("stall-old-client.txt"), [0; 64]).unwrap();
    recorder.capture("test.eviction", "save", 2000);
    recorder.wait_for_captures();
    recorder.flush();
    let records = lines(&perf_dir(&dir).join("client.jsonl"));
    assert!(records
        .iter()
        .any(|r| r["kind"] == "capture_evicted" && r["bytes"] == 64 && r["reason"] == "bytes"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn run_start_record_identifies_the_compiled_binary() {
    if isolated_lifecycle_test("run_start_record_identifies_the_compiled_binary") {
        return;
    }
    let dir = crate::utils::test_temp::unique_temp_path("freeze-run-start");
    let running = start(
        &dir,
        "client",
        &options(),
        PerfHooks {
            sample: no_sample,
            machine: || None,
            capture_stacks: no_capture,
        },
    )
    .unwrap();
    drop(running);
    let records = lines(&perf_dir(&dir).join("client.jsonl"));
    let starts: Vec<_> = records
        .iter()
        .filter(|r| r["kind"] == "run_start")
        .collect();
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0]["build_sha"], env!("BUS_BUILD_SHA"));
    assert_eq!(starts[0]["profile"], env!("BUS_BUILD_PROFILE"));
    assert_eq!(starts[0]["pid"], std::process::id());
    assert_eq!(starts[0]["build_sha"].as_str().unwrap().len(), 40);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn active_message_metadata_is_preserved_in_windows_and_stall_records() {
    let _serial = crate::utils::perf::operation::TEST_LOCK.lock().unwrap();
    let (mut recorder, dir) = recorder(
        "freeze-active-message",
        PerfHooks {
            sample: no_sample,
            machine: || None,
            capture_stacks: no_capture,
        },
    );
    let active = crate::utils::perf::operation::message_enter(
        crate::utils::perf::operation::Variant::EndpointControl,
        crate::utils::perf::operation::Phase::Endpoint,
        65536,
    );
    let expected = json!({"variant":"EndpointControl", "phase":"endpoint", "payload_bytes":65536});
    assert_eq!(
        recorder.window_record(Instant::now(), None, &Registry::new())["active_message"],
        expected
    );
    let event = StallEvent {
        kind: StallKind::Began,
        loop_name: "client.ui_loop",
        stage: "endpoint",
        busy_ms: 4000,
        stall_id: 5,
    };
    assert_eq!(recorder.stall_record(&event)["active_message"], expected);
    drop(active);
    assert!(recorder
        .window_record(Instant::now(), None, &Registry::new())
        .get("active_message")
        .is_none());
    std::fs::remove_dir_all(dir).unwrap();
}
