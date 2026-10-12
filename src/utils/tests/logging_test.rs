use super::*;

fn temp_log_path(name: &str) -> PathBuf {
    let unique = format!(
        "bus-logging-tests-{}-{}-{}",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    std::env::temp_dir().join(unique).join("bus.log")
}

#[test]
fn rotated_log_path_appends_numeric_suffix() {
    let path = PathBuf::from("/tmp/bus.log");
    assert_eq!(rotated_log_path(&path, 2), PathBuf::from("/tmp/bus.log.2"));
}

#[test]
fn rotate_files_shifts_existing_generations() {
    let path = temp_log_path("rotate");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "current").unwrap();
    fs::write(rotated_log_path(&path, 1), "older").unwrap();

    let mut state = RotatingFileState {
        path: path.clone(),
        max_bytes: 128,
        retention: Retention::Generations(2),
        file: None,
        current_size: 0,
        disabled: false,
        archive_signal: None,
    };
    state.rotate_files().unwrap();

    assert_eq!(
        fs::read_to_string(rotated_log_path(&path, 1)).unwrap(),
        "current"
    );
    assert_eq!(
        fs::read_to_string(rotated_log_path(&path, 2)).unwrap(),
        "older"
    );
    assert!(!path.exists());

    let _ = fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn write_replaces_log_without_retained_files_when_size_limit_is_reached() {
    let path = temp_log_path("replace");
    let dir = path.parent().unwrap().to_path_buf();
    fs::create_dir_all(&dir).unwrap();

    let options = LoggingOptions {
        filter: EnvFilter::new("bus=info"),
        max_bytes: 8,
        retention: Retention::Generations(0),
        dev: false,
    };
    let writer = RotatingFileMakeWriter::new(dir.clone(), "bus.log", &options).unwrap();
    {
        let mut guard = writer.make_writer();
        guard.write_all(b"12345678").unwrap();
        guard.write_all(b"abc").unwrap();
        guard.flush().unwrap();
    }

    assert_eq!(fs::read_to_string(&path).unwrap(), "abc");
    assert!(!rotated_log_path(&path, 1).exists());

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn supplied_rotation_options_keep_only_requested_generations() {
    let path = temp_log_path("options-rotation");
    let dir = path.parent().unwrap().to_path_buf();
    let options = LoggingOptions {
        filter: EnvFilter::new("bus=info"),
        max_bytes: 8,
        retention: Retention::Generations(2),
        dev: false,
    };
    let writer = RotatingFileMakeWriter::new(dir.clone(), "bus.log", &options).unwrap();
    {
        let mut guard = writer.make_writer();
        for bytes in [b"11111111", b"22222222", b"33333333", b"44444444"] {
            guard.write_all(bytes).unwrap();
        }
        guard.flush().unwrap();
    }
    drop(writer);

    assert_eq!(fs::read_to_string(&path).unwrap(), "44444444");
    assert_eq!(
        fs::read_to_string(rotated_log_path(&path, 1)).unwrap(),
        "33333333"
    );
    assert_eq!(
        fs::read_to_string(rotated_log_path(&path, 2)).unwrap(),
        "22222222"
    );
    assert!(!rotated_log_path(&path, 3).exists());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn supplied_filter_keeps_normal_events_and_selected_trace() {
    let path = temp_log_path("options-filter");
    let dir = path.parent().unwrap().to_path_buf();
    let options = LoggingOptions {
        filter: EnvFilter::new("bus=info,bus::logging_selected=trace"),
        max_bytes: DEFAULT_MAX_LOG_BYTES,
        retention: Retention::Generations(DEFAULT_RETAINED_LOG_FILES),
        dev: false,
    };
    let writer = RotatingFileMakeWriter::new(dir.clone(), "bus.log", &options).unwrap();
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(options.filter.clone())
        .with_writer(writer)
        .with_ansi(false)
        .without_time()
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(target: "bus::logging_normal", "NORMAL_INFO");
        tracing::debug!(target: "bus::logging_normal", "HIDDEN_DEBUG");
        tracing::trace!(target: "bus::logging_selected", "SELECTED_TRACE");
    });

    let logs = fs::read_to_string(path).unwrap();
    assert!(
        logs.contains("NORMAL_INFO") && logs.contains("SELECTED_TRACE"),
        "{logs}"
    );
    assert!(!logs.contains("HIDDEN_DEBUG"), "{logs}");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn startup_records_explicit_dev_and_existing_metadata() {
    for dev in [false, true] {
        let options = LoggingOptions {
            filter: EnvFilter::new("bus=info"),
            max_bytes: DEFAULT_MAX_LOG_BYTES,
            retention: Retention::Generations(DEFAULT_RETAINED_LOG_FILES),
            dev,
        };
        let capture = test_capture::Capture::default();
        capture.run(|| startup("client", &options));
        let logs = capture.text();
        for field in [
            "bus::utils::logging",
            "app.startup",
            "subsystem=\"client\"",
            "outcome=\"started\"",
            &format!("pid={}", std::process::id()),
            &format!("dev={dev}"),
            &format!("version=\"{}\"", env!("CARGO_PKG_VERSION")),
            &format!("build_id={:?}", crate::utils::version::build_id()),
            "bus starting",
        ] {
            assert!(logs.contains(field), "missing {field}: {logs}");
        }
    }
}

#[test]
fn api_request_completion_respects_shared_logging_target() {
    let capture = test_capture::Capture::default();
    capture.run_filtered("bus::utils::logging=info", || {
        api_request_started("request-fixture", "workspace.focus", true);
        api_request_completed("request-fixture", "workspace.focus", "ok", true);
    });

    let logs = capture.text();
    assert!(logs.contains("api.request.start"), "{logs}");
    assert!(
        logs.contains("api.request.complete"),
        "the shared logging target must retain request completion: {logs}"
    );
}

const HOUR: Duration = Duration::from_secs(60 * 60);

fn archives(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name != "bus.log")
        .collect();
    names.sort();
    names
}

fn gunzip(path: &Path) -> String {
    let mut text = String::new();
    std::io::Read::read_to_string(
        &mut flate2::read::GzDecoder::new(File::open(path).unwrap()),
        &mut text,
    )
    .unwrap();
    text
}

#[test]
fn archive_rotation_keeps_the_full_log_as_a_gzip_archive() {
    let path = temp_log_path("archive-rotation");
    let dir = path.parent().unwrap().to_path_buf();
    let policy = ArchivePolicy {
        max_age: 24 * HOUR,
        max_total_bytes: 1024 * 1024,
    };
    let options = LoggingOptions {
        filter: EnvFilter::new("bus=info"),
        max_bytes: 8,
        retention: Retention::Archive(policy),
        dev: true,
    };
    let writer = RotatingFileMakeWriter::new(dir.clone(), "bus.log", &options).unwrap();
    {
        let mut guard = writer.make_writer();
        guard.write_all(b"11111111").unwrap();
        guard.write_all(b"22222222").unwrap();
        guard.flush().unwrap();
    }
    // The writer's worker compresses off the logging path.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !archives(&dir).iter().any(|name| name.ends_with(".gz"))
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }

    assert_eq!(fs::read_to_string(&path).unwrap(), "22222222");
    let names = archives(&dir);
    assert_eq!(names.len(), 1, "{names:?}");
    assert!(
        names[0].starts_with("bus.log.") && names[0].ends_with(".gz"),
        "{names:?}"
    );
    assert_eq!(gunzip(&dir.join(&names[0])), "11111111");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn archive_maintenance_keeps_the_retention_window_and_drops_older_archives() {
    let path = temp_log_path("archive-age");
    let dir = path.parent().unwrap().to_path_buf();
    fs::create_dir_all(&dir).unwrap();
    let now = SystemTime::now();
    let policy = ArchivePolicy {
        max_age: 24 * HOUR,
        max_total_bytes: 1024 * 1024,
    };
    for (hours, text) in [(1, "recent"), (23, "a day old"), (25, "too old")] {
        fs::write(archive_path(&path, now - hours * HOUR), text).unwrap();
    }
    // An interrupted compression from an earlier process.
    fs::write(
        format!("{}.gz.tmp", archive_path(&path, now - 2 * HOUR).display()),
        "partial",
    )
    .unwrap();
    maintain_archives(&path, &policy, now);

    let names = archives(&dir);
    assert_eq!(names.len(), 2, "{names:?}");
    let texts: Vec<String> = names.iter().map(|name| gunzip(&dir.join(name))).collect();
    assert_eq!(texts, ["a day old", "recent"], "{names:?}");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn archive_maintenance_drops_the_oldest_archives_past_the_hard_cap() {
    let path = temp_log_path("archive-cap");
    let dir = path.parent().unwrap().to_path_buf();
    fs::create_dir_all(&dir).unwrap();
    let now = SystemTime::now();
    let mut sizes = Vec::new();
    for hours in [3, 2, 1] {
        let archive = archive_path(&path, now - hours * HOUR);
        fs::write(&archive, format!("archive from {hours}h ago")).unwrap();
        maintain_archives(
            &path,
            &ArchivePolicy {
                max_age: 24 * HOUR,
                max_total_bytes: u64::MAX,
            },
            now,
        );
        sizes.push(
            fs::metadata(format!("{}.gz", archive.display()))
                .unwrap()
                .len(),
        );
    }
    // Room for the two newest only.
    let policy = ArchivePolicy {
        max_age: 24 * HOUR,
        max_total_bytes: sizes[1] + sizes[2],
    };
    maintain_archives(&path, &policy, now);

    let names = archives(&dir);
    let texts: Vec<String> = names.iter().map(|name| gunzip(&dir.join(name))).collect();
    assert_eq!(
        texts,
        ["archive from 2h ago", "archive from 1h ago"],
        "{names:?}"
    );
    let _ = fs::remove_dir_all(dir);
}

/// The live logs had no startup line left to show which filter and retention
/// the process ran with; the line now records both.
#[test]
fn startup_records_the_effective_log_filter_and_retention() {
    let options = LoggingOptions {
        filter: EnvFilter::new(DEV_FILTER),
        max_bytes: DEV_MAX_LOG_BYTES,
        retention: Retention::Archive(DEV_LOG_ARCHIVE),
        dev: true,
    };
    let capture = test_capture::Capture::default();
    capture.run(|| startup("server", &options));
    let logs = capture.text();
    assert!(logs.contains("bus::protocol::keys::host=off"), "{logs}");
    assert!(logs.contains("bus=trace"), "{logs}");
    assert!(logs.contains("log_retention=Archive"), "{logs}");
}
