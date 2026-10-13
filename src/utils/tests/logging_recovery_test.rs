use super::super::*;
use super::{archives, gunzip, temp_log_path, wait_for_compressed_archives};

#[test]
fn log_writer_recovers_after_a_temporary_path_failure() {
    let path = temp_log_path("temporary-path-failure");
    let dir = path.parent().unwrap().to_path_buf();
    let options = LoggingOptions {
        filter: EnvFilter::new("bus=info"),
        max_bytes: 16,
        retention: Retention::Archive(DEV_LOG_ARCHIVE),
        dev: true,
    };
    let writer = RotatingFileMakeWriter::new(dir.clone(), "bus.log", &options).unwrap();
    writer.make_writer().write_all(b"before failure\n").unwrap();
    // The old open inode survives, but the replacement path is not writable.
    let saved = dir.join("saved.log");
    fs::rename(&path, &saved).unwrap();
    fs::create_dir(&path).unwrap();
    writer.make_writer().write_all(b"during failure\n").unwrap();
    fs::remove_dir(&path).unwrap();
    fs::rename(&saved, &path).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        writer.make_writer().write_all(b"recovered\n").unwrap();
        if fs::read_to_string(&path).unwrap().contains("recovered") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "logging never resumed after repairing the path"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_short_retention_client_cannot_erase_a_dev_log_stream() {
    // The short-retention writer may attach before or after the dev client.
    for release_first in [false, true] {
        let path = temp_log_path("mixed-retention");
        let dir = path.parent().unwrap().to_path_buf();
        let dev = LoggingOptions {
            filter: EnvFilter::new("bus=info"),
            max_bytes: 8,
            retention: Retention::Archive(DEV_LOG_ARCHIVE),
            dev: true,
        };
        let release = LoggingOptions {
            filter: EnvFilter::new("bus=info"),
            max_bytes: 8,
            retention: Retention::Generations(0),
            dev: false,
        };
        let (first, second) = if release_first {
            let short = RotatingFileMakeWriter::new(dir.clone(), "bus.log", &release).unwrap();
            let long = RotatingFileMakeWriter::new(dir.clone(), "bus.log", &dev).unwrap();
            (long, short)
        } else {
            let long = RotatingFileMakeWriter::new(dir.clone(), "bus.log", &dev).unwrap();
            let short = RotatingFileMakeWriter::new(dir.clone(), "bus.log", &release).unwrap();
            (long, short)
        };
        first.make_writer().write_all(b"DEVLINE\n").unwrap();
        second.make_writer().write_all(b"RELEASE\n").unwrap();
        second.make_writer().write_all(b"MORELOG\n").unwrap();
        first.make_writer().write_all(b"NEXTDEV\n").unwrap();
        wait_for_compressed_archives(&dir);
        let mut kept = fs::read_to_string(&path).unwrap();
        for name in archives(&dir) {
            kept.push_str(&gunzip(&dir.join(name)));
        }
        let _ = fs::remove_dir_all(dir);
        for line in ["DEVLINE\n", "RELEASE\n", "MORELOG\n", "NEXTDEV\n"] {
            assert_eq!(
                kept.matches(line).count(),
                1,
                "release_first={release_first}: {kept}"
            );
        }
    }
}

#[test]
fn archival_enablement_during_a_short_retention_rotation_preserves_existing_records() {
    let path = temp_log_path("archival-enablement-during-rotation");
    let dir = path.parent().unwrap().to_path_buf();
    let release = LoggingOptions {
        filter: EnvFilter::new("bus=info"),
        max_bytes: 8,
        retention: Retention::Generations(0),
        dev: false,
    };
    let short = RotatingFileMakeWriter::new(dir.clone(), "bus.log", &release).unwrap();
    short.make_writer().write_all(b"KEEPME\n").unwrap();
    let mut state = short.state.lock().unwrap();
    let shared = state.lock_live_file().unwrap();
    // Pause the real prewrite phases after the retention check, before rotation.
    state.inherit_archive_mode();
    assert!(matches!(state.retention, Retention::Generations(0)));
    let marker = state.archive_mode_path();
    let attach_dir = dir.clone();
    let (ready_tx, ready_rx) = mpsc::channel();
    let attaching = std::thread::spawn(move || {
        let dev = LoggingOptions {
            filter: EnvFilter::new("bus=info"),
            max_bytes: 8,
            retention: Retention::Archive(DEV_LOG_ARCHIVE),
            dev: true,
        };
        let long = RotatingFileMakeWriter::new(attach_dir, "bus.log", &dev).unwrap();
        ready_tx.send(()).unwrap();
        long
    });
    // A promotion serialized with the existing append may wait for this lock.
    let _ = ready_rx.recv_timeout(Duration::from_millis(200));
    let archive_enabled_before_rotation = marker.is_file();
    state.rotate_files().unwrap();
    state.open_current_file().unwrap();
    drop(shared);
    drop(state);
    let long = attaching.join().unwrap();
    long.make_writer().write_all(b"AFTER\n").unwrap();
    let mut kept = fs::read_to_string(&path).unwrap();
    for archive in archived_files(&path) {
        if archive
            .extension()
            .is_some_and(|extension| extension == "gz")
        {
            kept.push_str(&gunzip(&archive));
        } else {
            kept.push_str(&fs::read_to_string(&archive).unwrap());
        }
    }
    drop(long);
    drop(short);
    let _ = fs::remove_dir_all(dir);
    assert!(
        !archive_enabled_before_rotation || kept.contains("KEEPME\n"),
        "a short-retention rotation deleted records after archival was enabled: {kept:?}"
    );
}
