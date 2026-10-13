use super::*;

#[path = "logging_lock_test.rs"]
mod lock_tests;
#[path = "logging_recovery_test.rs"]
mod recovery_tests;
#[path = "logging_retry_test.rs"]
mod retry_tests;

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
        retry_at: None,
        archive_signal: None,
        archive_enabled: false,
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

/// Old numbered rotations must not sit outside the archive policy forever.
#[test]
fn archive_startup_recovers_numbered_leftovers_and_interrupted_compression() {
    let path = temp_log_path("startup-leftovers");
    let dir = path.parent().unwrap();
    fs::create_dir_all(dir).unwrap();
    let numbered = dir.join("bus.log.1");
    fs::write(&numbered, "legacy log").unwrap();
    let at = SystemTime::now() - HOUR;
    let plain = archive_path(&path, at);
    fs::write(&plain, "interrupted log").unwrap();
    let staging = PathBuf::from(format!("{}.gz.999999-0.tmp", plain.display()));
    fs::write(&staging, "partial gzip").unwrap();
    let state =
        RotatingFileState::open(path.clone(), 16, Retention::Archive(DEV_LOG_ARCHIVE)).unwrap();
    wait_for_compressed_archives(dir);
    assert!(!numbered.exists(), "numbered rotation was ignored");
    assert!(!plain.exists());
    assert!(!staging.exists());
    assert_eq!(gunzip(&dir.join("bus.log.1.gz")), "legacy log");
    assert_eq!(
        gunzip(&PathBuf::from(format!("{}.gz", plain.display()))),
        "interrupted log"
    );
    drop(state);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn archive_numbered_leftovers_count_toward_age_and_uncompressed_cap() {
    let path = temp_log_path("numbered-policy");
    let dir = path.parent().unwrap();
    fs::create_dir_all(dir).unwrap();
    let now = SystemTime::now();
    let old = dir.join("bus.log.2");
    let recent = dir.join("bus.log.1");
    fs::write(&old, "old log").unwrap();
    File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(now - 73 * HOUR))
        .unwrap();
    fs::write(&recent, "12345678").unwrap();
    let policy = ArchivePolicy {
        max_age: 72 * HOUR,
        max_total_bytes: 7,
    };
    // The same pre-compression pruning used by rotation must count all bytes.
    prune_archives(list_archives(&path), &policy, now);
    assert!(!old.exists(), "expired numbered rotation was ignored");
    assert!(
        !recent.exists(),
        "plain numbered rotation escaped the byte cap"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn archive_numbered_compression_preserves_age_for_later_maintenance() {
    let path = temp_log_path("numbered-age");
    let dir = path.parent().unwrap();
    fs::create_dir_all(dir).unwrap();
    let now = SystemTime::now();
    let plain = dir.join("bus.log.1");
    fs::write(&plain, "legacy log").unwrap();
    File::options()
        .write(true)
        .open(&plain)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(now - 71 * HOUR))
        .unwrap();
    maintain_archives(&path, &DEV_LOG_ARCHIVE, now);
    assert!(!plain.exists(), "numbered rotation was not compressed");
    assert_eq!(gunzip(&dir.join("bus.log.1.gz")), "legacy log");
    maintain_archives(&path, &DEV_LOG_ARCHIVE, now + 2 * HOUR);
    assert!(
        archives(dir).is_empty(),
        "compression reset the retention age"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn archive_numbered_reader_keeps_distinct_files_with_the_same_age() {
    let path = temp_log_path("numbered-reader");
    let dir = path.parent().unwrap();
    fs::create_dir_all(dir).unwrap();
    let at = SystemTime::now() - HOUR;
    for name in ["bus.log.1", "bus.log.2"] {
        let plain = dir.join(name);
        fs::write(&plain, name).unwrap();
        File::options()
            .write(true)
            .open(&plain)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(at))
            .unwrap();
    }
    let gz = compress_archive(&dir.join("bus.log.1")).unwrap();
    // A half-finished compression and unrelated prefix matches.
    fs::write(dir.join("bus.log.1"), "bus.log.1").unwrap();
    fs::write(dir.join("bus.log.0"), "not an archive").unwrap();
    fs::write(dir.join("bus.log.notes"), "not an archive").unwrap();
    let files = archived_files(&path);
    assert_eq!(
        files.len(),
        2,
        "distinct generations were ignored: {files:?}"
    );
    assert!(files.contains(&gz));
    assert!(files.contains(&dir.join("bus.log.2")));
    maintain_archives(&path, &DEV_LOG_ARCHIVE, SystemTime::now());
    assert!(dir.join("bus.log.0").exists());
    assert!(dir.join("bus.log.notes").exists());
    fs::remove_dir_all(dir).unwrap();
}

fn archives(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name != "bus.log" && name != ".bus.log.archive-mode")
        .collect();
    names.sort();
    names
}

/// Waits until the worker finished every archive: a `.gz` exists and no plain
/// or staging archive is left.
fn wait_for_compressed_archives(dir: &Path) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        let names = archives(dir);
        if names.iter().any(|name| name.ends_with(".gz"))
            && names.iter().all(|name| name.ends_with(".gz"))
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
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
    wait_for_compressed_archives(&dir);

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

#[test]
fn archive_cleanup_preserves_non_archive_temporary_files() {
    let path = temp_log_path("unrelated-temporary");
    let dir = path.parent().unwrap();
    fs::create_dir_all(dir).unwrap();
    let unrelated = dir.join("bus.log.user-backup.gz.tmp");
    fs::write(&unrelated, "user data").unwrap();
    maintain_archives(&path, &DEV_LOG_ARCHIVE, SystemTime::now());
    let preserved = fs::read_to_string(&unrelated).ok();
    let _ = fs::remove_dir_all(dir);
    assert_eq!(preserved.as_deref(), Some("user data"));
}

#[test]
fn archive_rotation_enforces_cap_before_background_maintenance() {
    let path = temp_log_path("pending-archive-cap");
    let dir = path.parent().unwrap();
    fs::create_dir_all(dir).unwrap();
    // Hold worker notifications pending, as when compression is busy. The
    // writer's real rotation path must enforce the cap without worker progress.
    let (signal, _pending) = mpsc::channel();
    let policy = ArchivePolicy {
        max_age: 72 * HOUR,
        max_total_bytes: 16,
    };
    let mut state = RotatingFileState {
        path: path.clone(),
        max_bytes: 8,
        retention: Retention::Archive(policy),
        file: None,
        current_size: 0,
        retry_at: None,
        archive_signal: Some(signal),
        archive_enabled: false,
    };
    for _ in 0..4 {
        state.rotate_if_needed(8).unwrap();
        state.file.as_mut().unwrap().write_all(b"12345678").unwrap();
        state.current_size += 8;
    }
    let archive_bytes: u64 = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.path() != path)
        .map(|entry| entry.metadata().unwrap().len())
        .sum();
    drop(state);
    let _ = fs::remove_dir_all(dir);
    assert!(
        archive_bytes <= policy.max_total_bytes,
        "pending archives use {archive_bytes} bytes, cap {} (active file excluded)",
        policy.max_total_bytes
    );
}

/// Two `bus client` processes attached to one session share `herdr-client.log`.
/// When one rotates it, the other keeps appending to the renamed file until its
/// own size check fires; those lines must survive archiving.
#[test]
fn archive_rotation_keeps_lines_a_second_writer_appends_after_the_rename() {
    let path = temp_log_path("archive-two-writers");
    let dir = path.parent().unwrap().to_path_buf();
    let options = |max_bytes| LoggingOptions {
        filter: EnvFilter::new("bus=info"),
        max_bytes,
        retention: Retention::Archive(ArchivePolicy {
            max_age: 24 * HOUR,
            max_total_bytes: 1024 * 1024,
        }),
        dev: true,
    };
    let rotating = RotatingFileMakeWriter::new(dir.clone(), "bus.log", &options(8)).unwrap();
    let other = RotatingFileMakeWriter::new(dir.clone(), "bus.log", &options(1024)).unwrap();
    {
        let mut guard = rotating.make_writer();
        guard.write_all(b"AAAAAAAA").unwrap();
        guard.write_all(b"BBBBBBBB").unwrap();
    }
    wait_for_compressed_archives(&dir);
    other.make_writer().write_all(b"OTHER_LINE").unwrap();

    let mut kept = fs::read_to_string(&path).unwrap();
    for name in archives(&dir) {
        kept.push_str(&gunzip(&dir.join(name)));
    }
    assert!(kept.contains("OTHER_LINE"), "kept: {kept:?}");
    let _ = fs::remove_dir_all(dir);
}

/// Every `bus client` attached to a session runs its own archive worker over
/// the shared `herdr-client.log` archives; concurrent maintenance passes must
/// not lose or truncate an archive.
#[test]
fn concurrent_archive_maintenance_keeps_every_archive_intact() {
    let path = temp_log_path("archive-concurrent-workers");
    let dir = path.parent().unwrap().to_path_buf();
    fs::create_dir_all(&dir).unwrap();
    let policy = ArchivePolicy {
        max_age: 24 * HOUR,
        max_total_bytes: u64::MAX,
    };
    let now = SystemTime::now();
    let mut lost = Vec::new();
    for round in 0..40u32 {
        let mut originals = Vec::new();
        for index in 0..4u32 {
            let archive = archive_path(
                &path,
                now - Duration::from_secs(u64::from(round * 10 + index + 1)),
            );
            let text = format!("round {round} archive {index} ").repeat(20_000);
            fs::write(&archive, &text).unwrap();
            originals.push((archive, text));
        }
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let workers: Vec<_> = (0..3)
            .map(|_| {
                let (barrier, path) = (Arc::clone(&barrier), path.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    maintain_archives(&path, &policy, now);
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        maintain_archives(&path, &policy, now);
        for (archive, text) in originals {
            let gz = PathBuf::from(format!("{}.gz", archive.display()));
            let kept = if archive.exists() {
                fs::read_to_string(&archive).ok()
            } else {
                File::open(&gz).ok().and_then(|file| {
                    let mut out = String::new();
                    std::io::Read::read_to_string(&mut flate2::read::GzDecoder::new(file), &mut out)
                        .ok()
                        .map(|_| out)
                })
            };
            if kept.as_deref() != Some(text.as_str()) {
                lost.push(format!("{}", gz.display()));
            }
        }
    }
    let _ = fs::remove_dir_all(&dir);
    assert!(lost.is_empty(), "archives lost or corrupted: {lost:?}");
}

/// A process that stopped between the archive rename and removing the plain
/// file leaves both; the next pass must count that archive once, not evict it.
#[test]
fn archive_maintenance_counts_a_half_finished_compression_once() {
    let path = temp_log_path("archive-half-compressed");
    let dir = path.parent().unwrap().to_path_buf();
    fs::create_dir_all(&dir).unwrap();
    let now = SystemTime::now();
    let archive = archive_path(&path, now - HOUR);
    let text = "only archive ".repeat(1000);
    fs::write(&archive, &text).unwrap();
    let gz = compress_archive(&archive).unwrap();
    fs::write(&archive, &text).unwrap();
    let size = fs::metadata(&gz).unwrap().len();
    let policy = ArchivePolicy {
        max_age: 24 * HOUR,
        max_total_bytes: size + size / 2,
    };
    maintain_archives(&path, &policy, now);
    let kept: Vec<String> = archives(&dir)
        .iter()
        .map(|name| gunzip(&dir.join(name)))
        .collect();
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(
        kept,
        [text],
        "the only archive, well under the cap, was evicted"
    );
}

#[cfg(unix)]
#[test]
fn archive_prewrite_check_preserves_lines_if_rotation_happens_before_append() {
    let path = temp_log_path("rotation-after-prewrite-check");
    let dir = path.parent().unwrap().to_path_buf();
    let options = |max_bytes| LoggingOptions {
        filter: EnvFilter::new("bus=info"),
        max_bytes,
        retention: Retention::Archive(ArchivePolicy {
            max_age: 24 * HOUR,
            max_total_bytes: 1024 * 1024,
        }),
        dev: true,
    };
    let rotating = RotatingFileMakeWriter::new(dir.clone(), "bus.log", &options(8)).unwrap();
    let other = RotatingFileMakeWriter::new(dir.clone(), "bus.log", &options(1024)).unwrap();
    rotating.make_writer().write_all(b"AAAAAAAA").unwrap();
    // Exercise the phases of RotatingFileGuard::write (shared lock, preflight,
    // append) with another writer scheduled between preflight and append.
    // Holding this writer's mutex does not lock the other writer's state.
    let mut state = other.state.lock().unwrap();
    let shared = state.lock_live_file().unwrap();
    state.rotate_if_needed(b"OTHER_LINE".len() as u64).unwrap();
    let rotating_write = std::thread::spawn(move || {
        rotating.make_writer().write_all(b"BBBBBBBB").unwrap();
        rotating
    });
    std::thread::sleep(Duration::from_millis(200));
    state
        .file
        .as_mut()
        .unwrap()
        .write_all(b"OTHER_LINE")
        .unwrap();
    state.current_size += b"OTHER_LINE".len() as u64;
    drop(shared);
    drop(state);
    let rotating = rotating_write.join().unwrap();
    wait_for_compressed_archives(&dir);
    let mut kept = fs::read_to_string(&path).unwrap();
    for name in archives(&dir) {
        kept.push_str(&gunzip(&dir.join(name)));
    }
    drop(other);
    drop(rotating);
    let _ = fs::remove_dir_all(dir);
    assert_eq!(
        kept.matches("OTHER_LINE").count(),
        1,
        "each successfully appended line must survive exactly once: {kept:?}"
    );
}

#[test]
fn archive_shared_active_file_stays_within_its_size_budget() {
    let path = temp_log_path("shared-active-size-budget");
    let dir = path.parent().unwrap().to_path_buf();
    let options = LoggingOptions {
        filter: EnvFilter::new("bus=info"),
        max_bytes: 8,
        retention: Retention::Archive(ArchivePolicy {
            max_age: 24 * HOUR,
            max_total_bytes: 1024 * 1024,
        }),
        dev: true,
    };
    let first = RotatingFileMakeWriter::new(dir.clone(), "bus.log", &options).unwrap();
    let second = RotatingFileMakeWriter::new(dir.clone(), "bus.log", &options).unwrap();
    first.make_writer().write_all(b"AAAA").unwrap();
    second.make_writer().write_all(b"BBBB").unwrap();
    first.make_writer().write_all(b"CCCC").unwrap();
    let active_bytes = fs::metadata(&path).unwrap().len();
    drop(first);
    drop(second);
    let _ = fs::remove_dir_all(dir);
    assert!(
        active_bytes <= options.max_bytes,
        "shared active log uses {active_bytes} bytes, budget {}",
        options.max_bytes
    );
}

/// Two clients that both write and both rotate the shared log keep every line
/// exactly once across the live file and the archives.
#[test]
fn archive_rotation_by_two_writers_keeps_each_line_exactly_once() {
    let path = temp_log_path("archive-two-rotating-writers");
    let dir = path.parent().unwrap().to_path_buf();
    let options = LoggingOptions {
        filter: EnvFilter::new("bus=info"),
        max_bytes: 64,
        retention: Retention::Archive(ArchivePolicy {
            max_age: 24 * HOUR,
            max_total_bytes: u64::MAX,
        }),
        dev: true,
    };
    let first = RotatingFileMakeWriter::new(dir.clone(), "bus.log", &options).unwrap();
    let second = RotatingFileMakeWriter::new(dir.clone(), "bus.log", &options).unwrap();
    let mut expected = Vec::new();
    for index in 0..300 {
        let line = format!("line-{index:04}\n");
        let writer = if index % 3 == 0 { &first } else { &second };
        writer.make_writer().write_all(line.as_bytes()).unwrap();
        expected.push(line);
        std::thread::sleep(Duration::from_micros(50));
    }
    wait_for_compressed_archives(&dir);

    let mut kept = fs::read_to_string(&path).unwrap();
    for name in archives(&dir) {
        kept.push_str(&gunzip(&dir.join(name)));
    }
    let _ = fs::remove_dir_all(&dir);
    let wrong: Vec<_> = expected
        .iter()
        .filter(|line| kept.matches(line.as_str()).count() != 1)
        .collect();
    assert!(wrong.is_empty(), "lines lost or duplicated: {wrong:?}");
}

/// A writer waiting on the shared log lock while the holder renames the file
/// into an archive and starts a new live file must write to the new file once
/// the lock is released, not into the renamed archive.
#[test]
fn archive_writer_waiting_on_the_log_lock_follows_a_rename_by_the_holder() {
    let path = temp_log_path("lock-wait-follows-rename");
    let dir = path.parent().unwrap().to_path_buf();
    let options = LoggingOptions {
        filter: EnvFilter::new("bus=info"),
        max_bytes: 1024,
        retention: Retention::Archive(ArchivePolicy {
            max_age: 24 * HOUR,
            max_total_bytes: u64::MAX,
        }),
        dev: true,
    };
    let writer = RotatingFileMakeWriter::new(dir.clone(), "bus.log", &options).unwrap();
    writer.make_writer().write_all(b"OLD;").unwrap();

    let holder = OpenOptions::new().append(true).open(&path).unwrap();
    holder.lock().unwrap();
    let (done_tx, done_rx) = mpsc::channel();
    let waiting = std::thread::spawn(move || {
        writer.make_writer().write_all(b"WAITED;").unwrap();
        done_tx.send(()).unwrap();
        writer
    });
    std::thread::sleep(Duration::from_millis(200));
    let blocked = done_rx.try_recv().is_err();
    let archive = archive_path(&path, SystemTime::now());
    fs::rename(&path, &archive).unwrap();
    fs::write(&path, "NEW;").unwrap();
    holder.unlock().unwrap();
    let finished = done_rx.recv_timeout(Duration::from_secs(5)).is_ok();
    let writer = waiting.join().unwrap();

    let live = fs::read_to_string(&path).unwrap();
    let renamed = fs::read_to_string(&archive).unwrap_or_default();
    drop(writer);
    let _ = fs::remove_dir_all(&dir);
    assert!(blocked, "the writer must wait for the holder's lock");
    assert!(finished, "the writer must not stay blocked after release");
    assert_eq!(live, "NEW;WAITED;", "archive: {renamed:?}");
    assert_eq!(renamed, "OLD;");
}

/// An entry larger than the size limit rotates a non-empty log once and is
/// then written whole to the fresh file, never rotated again or dropped.
#[test]
fn archive_oversized_entry_rotates_once_and_is_kept_whole() {
    let path = temp_log_path("archive-oversized-entry");
    let dir = path.parent().unwrap().to_path_buf();
    let options = LoggingOptions {
        filter: EnvFilter::new("bus=info"),
        max_bytes: 8,
        retention: Retention::Archive(ArchivePolicy {
            max_age: 24 * HOUR,
            max_total_bytes: u64::MAX,
        }),
        dev: true,
    };
    let writer = RotatingFileMakeWriter::new(dir.clone(), "bus.log", &options).unwrap();
    writer.make_writer().write_all(b"OVERSIZED-FIRST").unwrap();
    writer.make_writer().write_all(b"OVERSIZED-SECOND").unwrap();
    wait_for_compressed_archives(&dir);

    let live = fs::read_to_string(&path).unwrap();
    let kept: Vec<String> = archives(&dir)
        .iter()
        .map(|name| gunzip(&dir.join(name)))
        .collect();
    drop(writer);
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(live, "OVERSIZED-SECOND");
    assert_eq!(kept, ["OVERSIZED-FIRST"]);
}
