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
        retained_files: 2,
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
    assert_eq!(
        CAPTURES.load(Ordering::SeqCst),
        before + 1,
        "cooldown holds"
    );
    drop(second);
    recorder.watch(start + Duration::from_secs(60), now_us(), &registry);

    let records = lines(&perf_dir(&dir).join("client.jsonl"));
    let stalls: Vec<_> = records.iter().filter(|r| r["kind"] == "stall").collect();
    assert_eq!(stalls.len(), 5, "{records:?}");
    assert_eq!(stalls[0]["state"], "began");
    assert_eq!(stalls[0]["loop"], "test.rec.loop");
    assert_eq!(stalls[0]["stage"], "present");
    assert!(stalls[0]["busy_ms"].as_u64().unwrap() >= 300);
    assert_eq!(stalls[1]["state"], "capture");
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
