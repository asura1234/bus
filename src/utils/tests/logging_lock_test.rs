use super::super::*;
use super::temp_log_path;

#[test]
fn rotation_handoff_rechecks_replacement_size_before_append() {
    let path = temp_log_path("replacement-lock-size");
    let dir = path.parent().unwrap().to_path_buf();
    let options = LoggingOptions {
        filter: EnvFilter::new("bus=info"),
        max_bytes: 8,
        retention: Retention::Archive(ArchivePolicy {
            max_age: Duration::from_secs(86400),
            max_total_bytes: 1024 * 1024,
        }),
        dev: true,
    };
    let writer = RotatingFileMakeWriter::new(dir.clone(), "bus.log", &options).unwrap();
    writer.make_writer().write_all(b"AAAAAAAA").unwrap();
    let mut state = writer.state.lock().unwrap();
    // The production write phases release the old inode's lock and acquire
    // the replacement's. A new client can fill the replacement in that gap.
    let shared = state.lock_live_file();
    state.rotate_if_needed(8).unwrap();
    assert!(!shared.covers(state.file.as_ref()));
    let newcomer = RotatingFileMakeWriter::new(dir.clone(), "bus.log", &options).unwrap();
    newcomer.make_writer().write_all(b"BBBBBBBB").unwrap();
    // Production continues through lock_for_append: relock the replacement,
    // refresh its size and decide again before appending.
    let replacement_lock = state.lock_for_append(8, shared).unwrap();
    state.file.as_mut().unwrap().write_all(b"CCCCCCCC").unwrap();
    state.current_size += 8;
    let active_bytes = fs::metadata(&path).unwrap().len();
    drop(replacement_lock);
    drop(state);
    drop(newcomer);
    drop(writer);
    let _ = fs::remove_dir_all(dir);
    assert!(
        active_bytes <= options.max_bytes,
        "replacement log grew to {active_bytes} bytes, budget {}",
        options.max_bytes
    );
}

#[test]
fn cleanup_preserves_timestamped_files_without_a_compressor_writer_id() {
    let path = temp_log_path("staging-writer-id-validation");
    let dir = path.parent().unwrap();
    fs::create_dir_all(dir).unwrap();
    let stamp = archive_path(&path, SystemTime::now());
    let unrelated = PathBuf::from(format!("{}.gz.user-backup.tmp", stamp.display()));
    fs::write(&unrelated, "user data").unwrap();
    remove_interrupted_compressions(&path);
    let preserved = fs::read_to_string(&unrelated).ok();
    let _ = fs::remove_dir_all(dir);
    assert_eq!(preserved.as_deref(), Some("user data"));
}

#[cfg(unix)]
#[test]
fn subprocess_log_lock_holder() {
    let Some(path) = std::env::var_os("BUS_TEST_LOG_LOCK_HOLDER") else {
        return;
    };
    let file = OpenOptions::new().append(true).open(&path).unwrap();
    file.lock().unwrap();
    use std::io::Read;
    println!("LOCKED");
    io::stdout().flush().unwrap();
    let mut byte = [0];
    let _ = io::stdin().read(&mut byte);
}

#[cfg(unix)]
#[test]
fn log_writer_resumes_after_its_lock_holder_is_killed() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    let path = temp_log_path("killed-log-lock-holder");
    let dir = path.parent().unwrap().to_path_buf();
    let options = LoggingOptions {
        filter: EnvFilter::new("bus=info"),
        max_bytes: 1024,
        retention: Retention::Archive(DEV_LOG_ARCHIVE),
        dev: true,
    };
    let writer = RotatingFileMakeWriter::new(dir.clone(), "bus.log", &options).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "utils::logging::tests::lock_tests::subprocess_log_lock_holder",
            "--nocapture",
        ])
        .env("BUS_TEST_LOG_LOCK_HOLDER", &path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut locked = false;
    for _ in 0..10 {
        let mut line = String::new();
        if output.read_line(&mut line).unwrap() == 0 {
            break;
        }
        if line.trim() == "LOCKED" {
            locked = true;
            break;
        }
    }
    if !locked {
        let _ = child.kill();
        let _ = child.wait();
    }
    assert!(locked, "child did not acquire log lock");
    let (done_tx, done_rx) = mpsc::channel();
    let waiting = std::thread::spawn(move || {
        writer.make_writer().write_all(b"AFTER_CRASH").unwrap();
        let _ = done_tx.send(());
        writer
    });
    let blocked = done_rx.recv_timeout(Duration::from_millis(100)).is_err();
    child.kill().unwrap();
    child.wait().unwrap();
    let resumed = done_rx.recv_timeout(Duration::from_secs(5)).is_ok();
    assert!(
        resumed,
        "logging did not resume after crashed holder exited"
    );
    let writer = waiting.join().unwrap();
    let kept = fs::read_to_string(&path).unwrap();
    drop(writer);
    let _ = fs::remove_dir_all(dir);
    assert!(blocked, "writer should wait while child holds the lock");
    assert_eq!(kept, "AFTER_CRASH");
}
