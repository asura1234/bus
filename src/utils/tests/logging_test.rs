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
        retained_files: 2,
        file: None,
        current_size: 0,
        disabled: false,
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
        retained_files: 0,
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
        retained_files: 2,
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
        retained_files: DEFAULT_RETAINED_LOG_FILES,
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
            retained_files: DEFAULT_RETAINED_LOG_FILES,
            dev,
        };
        let capture = test_capture::Capture::default();
        capture.run(|| startup("client", options.dev));
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
