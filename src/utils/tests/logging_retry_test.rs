use super::super::*;
use super::temp_log_path;

fn dev_options() -> LoggingOptions {
    LoggingOptions {
        filter: EnvFilter::new("bus=info"),
        max_bytes: 1024,
        retention: Retention::Archive(DEV_LOG_ARCHIVE),
        dev: true,
    }
}

#[test]
fn log_writer_resumes_after_a_write_error_on_its_open_descriptor() {
    let path = temp_log_path("descriptor-write-error");
    let dir = path.parent().unwrap().to_path_buf();
    let writer = RotatingFileMakeWriter::new(dir.clone(), "bus.log", &dev_options()).unwrap();
    writer.make_writer().write_all(b"before\n").unwrap();
    // A read-only descriptor makes the next append fail like a full disk would.
    writer.state.lock().unwrap().file = Some(File::open(&path).unwrap());
    writer.make_writer().write_all(b"failed\n").unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        writer.make_writer().write_all(b"resumed\n").unwrap();
        if fs::read_to_string(&path).unwrap().contains("resumed") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "logging never resumed after a write error"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn log_writer_started_while_the_log_path_is_unavailable_resumes_once_it_is_repaired() {
    let path = temp_log_path("unavailable-at-start");
    let dir = path.parent().unwrap().to_path_buf();
    fs::create_dir_all(&path).unwrap();
    let writer = RotatingFileMakeWriter::new(dir.clone(), "bus.log", &dev_options());
    fs::remove_dir(&path).unwrap();
    let writer = writer
        .expect("a transient open failure at startup leaves the process without a log writer");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        writer.make_writer().write_all(b"resumed\n").unwrap();
        if fs::read_to_string(&path).is_ok_and(|text| text.contains("resumed")) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "logging never resumed"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn short_retention_writer_keeps_dev_lines_written_after_it_opened() {
    // The release writer opened and filled past its own limit before the dev writer joined.
    let path = temp_log_path("release-full-before-dev");
    let dir = path.parent().unwrap().to_path_buf();
    let release = LoggingOptions {
        filter: EnvFilter::new("bus=info"),
        max_bytes: 8,
        retention: Retention::Generations(0),
        dev: false,
    };
    let short = RotatingFileMakeWriter::new(dir.clone(), "bus.log", &release).unwrap();
    short.make_writer().write_all(b"RELEASE1\n").unwrap();
    let long = RotatingFileMakeWriter::new(dir.clone(), "bus.log", &dev_options()).unwrap();
    long.make_writer().write_all(b"DEVLINE\n").unwrap();
    short.make_writer().write_all(b"RELEASE2\n").unwrap();
    let kept = fs::read_to_string(&path).unwrap();
    let _ = fs::remove_dir_all(dir);
    assert!(kept.contains("DEVLINE\n"), "{kept}");
}
