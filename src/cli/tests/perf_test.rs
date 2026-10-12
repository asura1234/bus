use super::*;
use serde_json::json;

fn clock(ts_ms: u64) -> String {
    format!("t{}", ts_ms / 1_000)
}

fn window(role: &str, ts_s: u64, extra: serde_json::Value) -> Value {
    let mut record = json!({
        "kind": "window", "role": role, "pid": 7, "ts_ms": ts_s * 1_000,
        "cpu_pct": 10.0, "rss_mb": 90.0, "fds": 19, "threads": 9,
        "load1": 3.5, "cpus": 12, "sys_cpu_pct": 20.0, "swap_used_mb": 100.0,
    });
    for (key, value) in extra.as_object().unwrap() {
        record[key] = value.clone();
    }
    record
}

#[test]
fn quiet_records_report_no_stalls_or_slow_windows_but_still_show_peaks() {
    let records = vec![
        window("client", 100, json!({})),
        window("server", 100, json!({})),
    ];
    let text = summarize(&records, clock);
    assert!(text.contains("stalls: none"), "{text}");
    assert!(text.contains("slow windows: none"), "{text}");
    assert!(text.contains("client pid 7 peaks:"), "{text}");
    assert!(
        text.contains("fds 19 at t100"),
        "counts print as integers: {text}"
    );
    assert!(text.contains("cpu 10.0% at t100"), "{text}");
    assert!(text.contains("server pid 7 peaks:"), "{text}");
}

#[test]
fn stalls_list_loop_stage_duration_and_stack_capture() {
    let records = vec![
        json!({"kind": "stall", "state": "began", "role": "client", "ts_ms": 200_000,
               "loop": "client.ui_loop", "stage": "present", "busy_ms": 260}),
        json!({"kind": "stall", "state": "moved", "role": "client", "ts_ms": 201_000,
               "loop": "client.ui_loop", "stage": "timer", "busy_ms": 1_000}),
        json!({"kind": "stall", "state": "capture", "role": "client", "ts_ms": 202_000,
               "loop": "client.ui_loop", "stage": "timer", "busy_ms": 2_000,
               "file": "stall-x-client.txt", "ok": true}),
        json!({"kind": "stall", "state": "capture", "role": "server", "ts_ms": 203_000,
               "loop": "server.loop", "stage": "render", "busy_ms": 2_000,
               "file": "stall-y-server.txt", "ok": false}),
        json!({"kind": "stall", "state": "ended", "role": "client", "ts_ms": 205_000,
               "loop": "client.ui_loop", "stage": "present", "busy_ms": 5_100}),
    ];
    let text = summarize(&records, clock);
    assert!(
        text.contains("t200 client client.ui_loop blocked in present"),
        "{text}"
    );
    assert!(
        text.contains("t205 client client.ui_loop resumed after 5100 ms (present)"),
        "{text}"
    );
    assert!(
        text.contains("t201 client client.ui_loop still blocked, now in timer (1000 ms so far)"),
        "{text}"
    );
    assert!(
        text.contains("stacks captured after 2000 ms in timer: perf/stall-x-client.txt"),
        "{text}"
    );
    assert!(
        text.contains("t203 server server.loop stack capture after 2000 ms in render failed"),
        "{text}"
    );
    assert!(
        !text.contains("stall-y-server.txt"),
        "a failed capture points at no file: {text}"
    );
}

#[test]
fn slow_windows_name_the_slowest_metrics_with_the_system_load_at_that_time() {
    let records = vec![
        window("client", 300, json!({})),
        window(
            "client",
            301,
            json!({
                "cpu_pct": 99.5, "load1": 43.0, "sys_cpu_pct": 99.7, "watchdog_lag_max_ms": 340.0,
                "timers": {
                    "client.ui_loop": {"n": 3, "avg_ms": 800.0, "max_ms": 2400.0, "ge16ms": 1, "ge100ms": 1, "ge1s": 1},
                    "coordinator.tick": {"n": 1, "avg_ms": 120.0, "max_ms": 120.0, "ge16ms": 1, "ge100ms": 1, "ge1s": 0},
                    "client.compose": {"n": 60, "avg_ms": 1.0, "max_ms": 3.0, "ge16ms": 0, "ge100ms": 0, "ge1s": 0}
                },
                "gauges": {"client.event_queue": {"max": 40, "last": 2}}
            }),
        ),
    ];
    let text = summarize(&records, clock);
    let line = text
        .lines()
        .find(|line| line.starts_with("  t301 client"))
        .unwrap_or_else(|| panic!("{text}"));
    assert!(line.contains("cpu 99.5%"), "{line}");
    assert!(line.contains("sys 99.7%"), "{line}");
    assert!(line.contains("load 43.0/12"), "{line}");
    assert!(line.contains("lag 340ms"), "{line}");
    assert!(line.contains("client.ui_loop max 2400ms"), "{line}");
    assert!(line.contains("coordinator.tick max 120ms"), "{line}");
    assert!(
        !line.contains("client.compose"),
        "fast timers are not culprits: {line}"
    );
    assert!(line.contains("client.event_queue 40"), "{line}");
    assert!(!text.contains("  t300 client"), "{text}");
}

#[test]
fn since_filter_and_rotated_files_are_read_in_time_order() {
    let dir = crate::utils::test_temp::unique_temp_path("bus-perf-cli");
    let perf = dir.join("perf");
    std::fs::create_dir_all(&perf).unwrap();
    let line = |ts: u64| format!("{}\n", window("server", ts, json!({})));
    // An archive the log writer already gzipped, and one it has not yet.
    let gz =
        std::fs::File::create(perf.join("server.jsonl.20260101T000000.000000000Z.gz")).unwrap();
    let mut encoder = flate2::write::GzEncoder::new(gz, flate2::Compression::default());
    std::io::Write::write_all(&mut encoder, line(7).as_bytes()).unwrap();
    encoder.finish().unwrap();
    std::fs::write(
        perf.join("server.jsonl.20260101T000001.000000000Z"),
        line(10),
    )
    .unwrap();
    std::fs::write(perf.join("server.jsonl"), line(20) + "not json\n").unwrap();
    std::fs::write(perf.join("client.jsonl"), line(5)).unwrap();

    let all = read_records(&dir, None);
    let times: Vec<_> = all.iter().map(|r| r["ts_ms"].as_u64().unwrap()).collect();
    assert_eq!(times, vec![5_000, 7_000, 10_000, 20_000]);
    let recent = read_records(&dir, Some(8_000));
    assert_eq!(recent.len(), 2);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn peaks_are_reported_per_process_when_clients_share_a_role_file() {
    let mut quiet = window("client", 100, json!({"cpu_pct": 5.0}));
    quiet["pid"] = json!(8);
    let records = vec![window("client", 100, json!({"cpu_pct": 80.0})), quiet];
    let text = summarize(&records, clock);
    let line = |pid: u64| {
        text.lines()
            .find(|line| line.starts_with(&format!("client pid {pid} peaks:")))
            .unwrap_or_else(|| panic!("{text}"))
            .to_owned()
    };
    assert!(line(7).contains("cpu 80.0%"), "{text}");
    assert!(line(8).contains("cpu 5.0%"), "{text}");
}

#[test]
fn since_argument_accepts_minutes_hours_and_rejects_garbage() {
    assert_eq!(parse_since(&[]), Ok(Some(30 * 60_000)));
    assert_eq!(
        parse_since(&["--since".into(), "90m".into()]),
        Ok(Some(90 * 60_000))
    );
    assert_eq!(
        parse_since(&["--since".into(), "2h".into()]),
        Ok(Some(2 * 3_600_000))
    );
    assert_eq!(parse_since(&["--all".into()]), Ok(None));
    assert!(parse_since(&["--since".into(), "soon".into()]).is_err());
    assert!(parse_since(&["--bogus".into()]).is_err());
}

#[test]
fn perf_reader_subprocess() {
    if std::env::var_os("BUS_PERF_READER_PROBE").is_none() {
        return;
    }
    crate::cli::run(&["perf".into(), "--all".into()]).unwrap();
}

#[test]
fn perf_reader_does_not_wait_for_the_session_registry_writer() {
    use crate::messaging::storage::{io, sessions::LocalSessionRegistry};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let home = crate::utils::test_temp::unique_temp_path("perf-reader-lock");
    let base = home.join(".local/share/bus");
    let registry = LocalSessionRegistry::new(base.clone());
    let session = registry.create().unwrap();
    std::fs::create_dir_all(session.root.join("perf")).unwrap();
    let lease = io::lock(&base.join("session-registry.lock")).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "cli::perf::tests::perf_reader_subprocess",
            "--nocapture",
        ])
        .env("HOME", &home)
        .env("BUS_PERF_READER_PROBE", "1")
        .env_remove("BUS_DATA_DIR")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    drop(lease);
    std::fs::remove_dir_all(home).unwrap();
    assert!(
        status.is_some_and(|status| status.success()),
        "bus perf must read evidence without waiting for session-registry.lock; status={status:?}"
    );
}

#[test]
fn perf_reader_preserves_plain_archives_beside_finished_gzip_copies() {
    let dir = crate::utils::test_temp::unique_temp_path("perf-reader-read-only");
    let perf = dir.join("perf");
    std::fs::create_dir_all(&perf).unwrap();
    let plain = perf.join("client.jsonl.20260101T000000.000000000Z");
    let compressed = perf.join("client.jsonl.20260101T000000.000000000Z.gz");
    let contents = format!("{}\n", window("client", 100, json!({})));
    std::fs::write(&plain, &contents).unwrap();
    let mut encoder = flate2::write::GzEncoder::new(
        std::fs::File::create(compressed).unwrap(),
        flate2::Compression::default(),
    );
    std::io::Write::write_all(&mut encoder, contents.as_bytes()).unwrap();
    encoder.finish().unwrap();
    let records = read_records(&dir, None);
    let retained_plain = std::fs::read_to_string(&plain).ok();
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(records.len(), 1, "finished compression is read once");
    assert_eq!(
        retained_plain.as_deref(),
        Some(contents.as_str()),
        "a files-only perf reader must leave archive maintenance to the writer"
    );
}
