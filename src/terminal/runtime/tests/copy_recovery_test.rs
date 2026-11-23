use super::*;
use std::{sync::Arc, time::Duration};

fn key(directory: &Path, snapshot: &str) -> Key {
    Key {
        directory: directory.to_owned(),
        digest: crate::utils::time::digest(snapshot.as_bytes()),
    }
}

fn context(key: &Key) -> Context {
    Context {
        key: key.clone(),
        span: tracing::Span::none(),
        dispatch: tracing::dispatcher::get_default(Clone::clone),
    }
}

fn service() -> (&'static Service, mpsc::Receiver<Work>) {
    let (work, receiver) = mpsc::channel();
    let service = Box::leak(Box::new(Service {
        work,
        registry: Default::default(),
    }));
    (service, receiver)
}

fn begin(service: &'static Service, key: &Key, snapshot: &str) -> Attempt {
    let context = context(key);
    let (registration, _) = register(service, &context, snapshot);
    Attempt {
        service,
        context: Some(context),
        registration,
    }
}

fn make_record(key: &Key, body: &str) -> PathBuf {
    crate::platform::fs::private_dir(&key.directory).unwrap();
    let path = key.directory.join(format!("copied-{}.txt", key.digest));
    crate::platform::create_private_state_file(&path)
        .unwrap()
        .write_all(body.as_bytes())
        .unwrap();
    path
}

fn pause_effect(path: &Path) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (started, waiting) = mpsc::channel();
    let (resume, resumed) = mpsc::channel();
    test_hooks::install_effect(path.to_owned(), move || {
        started.send(()).unwrap();
        resumed.recv_timeout(Duration::from_secs(5)).unwrap();
    });
    (waiting, resume)
}

#[test]
fn copy_recovery_round3_prune_rechecks_after_a_new_attempt_registers() {
    let dir = crate::utils::test_temp::unique_temp_path("copy-recheck-prune");
    let protected = key(&dir, "protected candidate");
    let oldest = make_record(&protected, "protected candidate");
    File::options()
        .write(true)
        .open(&oldest)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH)
        .unwrap();
    for index in 0..15 {
        let body = format!("older record {index}");
        make_record(&key(&dir, &body), &body);
    }
    let replacement = key(&dir, "durable replacement");
    let path = make_record(&replacement, "durable replacement");
    let (service, _receiver) = service();
    let (waiting, resume) = pause_effect(&oldest);
    let prune = thread::spawn(move || service.enforce_retention(&replacement, &path));
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    let active = begin(service, &protected, "protected candidate");
    resume.send(()).unwrap();
    prune.join().unwrap();
    assert_eq!(fs::read_to_string(oldest).unwrap(), "protected candidate");
    assert_eq!(records(&dir).unwrap().len(), 16);
    drop(active);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn copy_recovery_round3_retirement_rechecks_after_a_new_attempt_registers() {
    let dir = crate::utils::test_temp::unique_temp_path("copy-recheck-retire");
    let snapshot = key(&dir, "original draft");
    let path = make_record(&snapshot, "original draft");
    let (service, _receiver) = service();
    let old = begin(service, &snapshot, "original draft");
    old.send(Outcome::Restored);
    let (waiting, resume) = pause_effect(&path);
    let worker_key = snapshot.clone();
    let retiring = thread::spawn(move || service.settle(&worker_key));
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    let active = begin(service, &snapshot, "original draft");
    resume.send(()).unwrap();
    retiring.join().unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), "original draft");
    drop(active);
    fs::remove_dir_all(dir).unwrap();
}

#[derive(Clone)]
struct NoticeSink(Arc<Mutex<Vec<u8>>>);

impl Write for NoticeSink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn copy_recovery_round3_notice_rechecks_after_a_newer_restore() {
    let dir = crate::utils::test_temp::unique_temp_path("copy-recheck-notice");
    let snapshot = key(&dir, "original draft");
    let path = make_record(&snapshot, "original draft");
    let (service, _receiver) = service();
    let (waiting, resume) = pause_effect(&path);
    let worker_key = snapshot.clone();
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let sink = NoticeSink(buffer.clone());
    let dispatch = tracing::Dispatch::new(
        tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || sink.clone())
            .finish(),
    );
    tracing::dispatcher::with_default(&dispatch, || {
        begin(service, &snapshot, "original draft").send(Outcome::Retain {
            reason: "test_retention",
            error: String::new(),
        });
    });
    let notifying = thread::spawn(move || service.settle(&worker_key));
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    let later = begin(service, &snapshot, "original draft");
    later.send(Outcome::Restored);
    resume.send(()).unwrap();
    notifying.join().unwrap();
    let log = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();
    assert!(!log.contains("bus.draft.recovery_required"), "{log}");
    service.settle(&snapshot);
    assert!(!path.exists());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn copy_recovery_round3_deferred_prune_runs_after_unverified_controls() {
    let dir = crate::utils::test_temp::unique_temp_path("copy-deferred-prune");
    let (service, _receiver) = service();
    let mut controls = Vec::new();
    for index in 0..16 {
        let body = format!("pending text {index}");
        let digest = key(&dir, &body);
        make_record(&digest, "edited record bytes");
        let active = begin(
            service,
            &digest,
            &"x".repeat(crate::platform::draft_editor::MAX_BYTES as usize + 1),
        );
        controls.push(digest);
        active.send(Outcome::Retain {
            reason: "test_retention",
            error: String::new(),
        });
    }
    let replacement = key(&dir, "durable replacement");
    let path = make_record(&replacement, "durable replacement");
    service.enforce_retention(&replacement, &path);
    assert_eq!(records(&dir).unwrap().len(), 17);
    for key in controls {
        service.settle(&key);
    }
    assert_eq!(records(&dir).unwrap().len(), 16);
    assert!(path.is_file());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn copy_recovery_round3_stalled_worker_bounds_distinct_and_repeated_controls() {
    let dir = crate::utils::test_temp::unique_temp_path("copy-bounded-controls");
    let (service, receiver) = service();
    for index in 0..512 {
        let text = format!("distinct digest {index}");
        begin(service, &key(&dir, &text), &text).send(Outcome::Restored);
        begin(service, &key(&dir, "same key"), "same key").send(Outcome::Restored);
    }
    let entries = service.registry().entries.len();
    let queued = receiver.try_iter().count();
    assert!(
        entries <= 256 && queued <= 273,
        "entries={entries} enqueued={queued}"
    );
}

#[test]
fn copy_recovery_round3_disconnect_releases_pending_registry_state() {
    let dir = crate::utils::test_temp::unique_temp_path("copy-disconnected-controls");
    let (service, receiver) = service();
    let old = begin(service, &key(&dir, "admitted save"), "admitted save");
    drop(receiver);
    old.send(Outcome::Restored);
    begin(service, &key(&dir, "new attempt"), "new attempt").send(Outcome::Restored);
    let registry = service.registry();
    assert_eq!(registry.queued, 0);
    assert!(registry.entries.is_empty());
}

#[test]
fn copy_recovery_round3_worker_exit_releases_abandoned_saves() {
    let dir = crate::utils::test_temp::unique_temp_path("copy-worker-exit");
    let (service, receiver) = service();
    test_hooks::install(dir.clone(), || panic!("injected worker exit during save"));
    let first = begin(service, &key(&dir, "working save"), "working save");
    let queued = begin(service, &key(&dir, "abandoned save"), "abandoned save");
    assert!(thread::spawn(move || service.run(receiver)).join().is_err());

    fs::remove_dir_all(dir).unwrap();
    {
        let registry = service.registry();
        assert_eq!(registry.queued, 0);
        assert!(registry.entries.is_empty());
    }
    drop((first, queued));
}
