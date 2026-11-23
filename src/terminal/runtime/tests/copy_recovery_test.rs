use super::storage::records;
use super::*;
use std::{
    fs::{self, File},
    io::{self, Write},
    sync::Arc,
    time::Duration,
};

fn key(directory: &Path, snapshot: &str) -> Key {
    Key {
        directory: directory.to_owned(),
        digest: crate::utils::time::digest(snapshot.as_bytes()),
    }
}

fn context(key: &Key) -> Context {
    Context {
        key: key.clone(),
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

fn begin(service: &'static Service, key: &Key, snapshot: &str) -> Attempt<'static> {
    begin_with_diagnostics(service, key, snapshot, None)
}

fn begin_with_diagnostics<'a>(
    service: &'static Service,
    key: &Key,
    snapshot: &str,
    diagnostics: Option<&'a CallerDiagnostics>,
) -> Attempt<'a> {
    let context = context(key);
    let (registration, _) = register(service, &context, snapshot, diagnostics, None);
    Attempt {
        service,
        context: Some(context),
        registration,
        diagnostics,
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
    let (service, receiver) = service();
    let old = begin(service, &snapshot, "original draft");
    old.send(Outcome::Restored);
    service.step(receiver.recv().unwrap());
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
    let (service, receiver) = service();
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
    service.step(receiver.recv().unwrap());
    let notifying = thread::spawn(move || service.settle(&worker_key));
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    let later = begin(service, &snapshot, "original draft");
    later.send(Outcome::Restored);
    resume.send(()).unwrap();
    notifying.join().unwrap();
    let log = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();
    assert!(!log.contains("bus.draft.recovery_required"), "{log}");
    drain(service, &receiver);
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
        entries <= MAX_TRACKED && queued <= MAX_TRACKED + MAX_OVERFLOW + QUEUED_BACKUPS + 1,
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

fn dispatch_for(buffer: &Arc<Mutex<Vec<u8>>>) -> tracing::Dispatch {
    let sink = NoticeSink(buffer.clone());
    tracing::Dispatch::new(
        tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || sink.clone())
            .finish(),
    )
}

fn output(buffer: &Mutex<Vec<u8>>) -> String {
    String::from_utf8(buffer.lock().unwrap().clone()).unwrap()
}

fn retain() -> Outcome {
    Outcome::Retain {
        reason: "test_retention",
        error: String::new(),
    }
}

fn drain(service: &Service, receiver: &mpsc::Receiver<Work>) {
    while let Ok(work) = receiver.try_recv() {
        service.step(work);
    }
}

type NoticeBarrier = (mpsc::Sender<()>, mpsc::Receiver<()>);

#[derive(Clone)]
struct BlockingNotice {
    output: NoticeSink,
    barrier: Arc<Mutex<Option<NoticeBarrier>>>,
}

impl Write for BlockingNotice {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if String::from_utf8_lossy(bytes).contains("bus.draft.recovery_required") {
            let block = self.barrier.lock().unwrap().take();
            if let Some((started, resume)) = block {
                started.send(()).unwrap();
                resume.recv_timeout(Duration::from_secs(8)).unwrap();
            }
        }
        self.output.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn copy_recovery_round4_blocked_notice_leaves_second_pane_free_to_settle() {
    let dir = crate::utils::test_temp::unique_temp_path("copy-blocked-notice");
    let first = key(&dir.join("first"), "first draft");
    let second = key(&dir.join("second"), "second draft");
    make_record(&first, "first draft");
    let (service, receiver) = service();
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let (started, waiting) = mpsc::channel();
    let (resume, released) = mpsc::channel();
    let sink = BlockingNotice {
        output: NoticeSink(buffer.clone()),
        barrier: Arc::new(Mutex::new(Some((started, released)))),
    };
    let dispatch = tracing::Dispatch::new(
        tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || sink.clone())
            .finish(),
    );
    tracing::dispatcher::with_default(&dispatch, || {
        begin(service, &first, "first draft").send(retain());
    });
    service.step(receiver.recv().unwrap());
    let worker = thread::spawn(move || service.settle(&first));
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    let (done, delivered) = mpsc::channel();
    let foreground = thread::spawn(move || {
        begin(service, &second, "second draft").send(Outcome::Restored);
        done.send(()).unwrap();
    });

    let finished_before_unblock = delivered.recv_timeout(Duration::from_millis(500)).is_ok();
    resume.send(()).unwrap();
    worker.join().unwrap();
    foreground.join().unwrap();
    fs::remove_dir_all(dir).unwrap();
    assert!(
        finished_before_unblock,
        "second pane could not register and settle while the sink was blocked"
    );
    assert_eq!(
        output(&buffer)
            .matches("bus.draft.recovery_required")
            .count(),
        1
    );
}

#[test]
fn copy_recovery_round4_newer_retain_waits_for_its_admitted_save() {
    let dir = crate::utils::test_temp::unique_temp_path("copy-settle-before-save");
    let snapshot = key(&dir, "draft A and B");
    let (service, receiver) = service();
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let dispatch = dispatch_for(&buffer);
    test_hooks::install(dir.clone(), || Err(io::Error::other("first save failed")));
    tracing::dispatcher::with_default(&dispatch, || {
        begin(service, &snapshot, "draft A and B").send(retain());
    });
    // A's failed save queues its control. Register the newer admitted save
    // behind that control, so settlement must defer the newer resolution.
    service.step(receiver.recv().unwrap());
    tracing::dispatcher::with_default(&dispatch, || {
        begin(service, &snapshot, "draft A and B").send(retain());
    });
    drain(service, &receiver);
    let notices = output(&buffer);
    let path = verified_records(&snapshot).unwrap().pop().unwrap();
    fs::remove_dir_all(dir).unwrap();
    assert!(!notices.contains("bus.draft.copy_loss"), "{notices}");
    assert_eq!(
        notices.matches("bus.draft.recovery_required").count(),
        1,
        "{notices}"
    );
    assert!(notices.contains(&*path.to_string_lossy()), "{notices}");
}

fn fill_tracking(service: &'static Service, directory: &Path) -> Vec<Attempt<'static>> {
    (0..MAX_TRACKED)
        .map(|i| {
            let text = format!("held {i}");
            begin(service, &key(directory, &text), &text)
        })
        .collect()
}

#[test]
fn copy_recovery_round4_overflow_retain_pins_its_verified_file() {
    let dir = crate::utils::test_temp::unique_temp_path("copy-overflow-pin");
    let (service, receiver) = service();
    let held = fill_tracking(service, &dir.join("held"));
    // Remove the held payloads from the test channel without running them.
    // Their owners remain registered so the next distinct digest overflows.
    assert_eq!(receiver.try_iter().count(), 16);
    let pane = dir.join("pane");
    let snapshot = key(&pane, "overflow draft");
    let path = make_record(&snapshot, "overflow draft");
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH)
        .unwrap();
    for i in 0..15 {
        let body = format!("older {i}");
        make_record(&key(&pane, &body), &body);
    }
    let current = key(&pane, "durable replacement");
    let replacement = make_record(&current, "durable replacement");
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let dispatch = dispatch_for(&buffer);
    let overflow = tracing::dispatcher::with_default(&dispatch, || {
        begin(service, &snapshot, "overflow draft")
    });
    // Another attempt beyond the metadata limit blocks cap enforcement.
    // The first pass that can actually prune must still honor this pin.
    let overflow_held: Vec<_> = (0..MAX_OVERFLOW - 1)
        .map(|i| {
            let text = format!("reserved overflow {i}");
            begin(service, &key(&dir.join("held"), &text), &text)
        })
        .collect();
    let untracked = begin(
        service,
        &key(&dir.join("held"), "beyond both limits"),
        "beyond both limits",
    );
    service.enforce_retention(&current, &replacement);
    overflow.send(retain());
    drain(service, &receiver);
    assert_eq!(records(&pane).unwrap().len(), 17);
    untracked.send(Outcome::Restored);
    drain(service, &receiver);

    let contents = fs::read_to_string(&path);
    let log = output(&buffer);
    let count = records(&pane).unwrap().len();
    fs::remove_dir_all(dir).unwrap();
    assert_eq!(contents.unwrap(), "overflow draft");
    assert_eq!(count, 16);
    assert!(
        log.contains("bus.draft.recovery_required") && log.contains(&*path.to_string_lossy()),
        "{log}"
    );
    drop((held, overflow_held));
}

#[test]
fn copy_recovery_round4_overflow_restored_retires_verified_file() {
    let dir = crate::utils::test_temp::unique_temp_path("copy-overflow-retire");
    let (service, receiver) = service();
    let held = fill_tracking(service, &dir.join("held"));
    assert_eq!(receiver.try_iter().count(), 16);
    let snapshot = key(&dir.join("pane"), "overflow restored");
    let path = make_record(&snapshot, "overflow restored");
    begin(service, &snapshot, "overflow restored").send(Outcome::Restored);
    drain(service, &receiver);
    let remains = path.exists();
    fs::remove_dir_all(dir).unwrap();
    assert!(!remains, "overflow restore left a stale verified file");
    drop(held);
}

#[test]
fn copy_recovery_round4_deferred_directories_are_bounded_with_overlapping_overflow() {
    let dir = crate::utils::test_temp::unique_temp_path("copy-deferred-bound");
    let (service, _receiver) = service();
    let held = fill_tracking(service, &dir.join("held"));
    let overflow_held: Vec<_> = (0..MAX_OVERFLOW)
        .map(|i| {
            let text = format!("overflow held {i}");
            begin(service, &key(&dir.join("held"), &text), &text)
        })
        .collect();
    let mut overflow = begin(service, &key(&dir, "always active"), "always active");
    for index in 0..72 {
        let next = begin(service, &key(&dir, "overlapping"), "overlapping");
        overflow.send(Outcome::Restored);
        overflow = next;
        let pane = dir.join(index.to_string());
        for i in 0..17 {
            let body = format!("record {i}");
            make_record(&key(&pane, &body), &body);
        }
        let current = key(&pane, "record 16");
        let path = verified_records(&current).unwrap().pop().unwrap();
        service.enforce_retention(&current, &path);
    }
    let deferred = service.registry().deferred.len();
    fs::remove_dir_all(dir).unwrap();
    assert!(deferred <= 64, "deferred directories={deferred}");
    drop((overflow, overflow_held, held));
}

#[test]
fn copy_recovery_round4_disconnect_reports_abandoned_outcomes_once() {
    let dir = crate::utils::test_temp::unique_temp_path("copy-disconnect-log");
    let (service, receiver) = service();
    let snapshot = key(&dir, "restored; private content");
    make_record(&snapshot, "restored; private content");
    for i in 0..16 {
        let body = format!("pending {i}");
        make_record(&key(&dir, &body), &body);
    }
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let dispatch = dispatch_for(&buffer);
    tracing::dispatcher::with_default(&dispatch, || {
        begin(service, &snapshot, "restored; private content").send(Outcome::Restored);
        service.registry().deferred.push_back((
            dir.clone(),
            verified_records(&snapshot).unwrap().pop().unwrap(),
        ));
        let last = begin(service, &key(&dir, "last pending"), "last pending");
        drop(receiver);
        last.send(retain());
        let diagnostics = CallerDiagnostics::default();
        begin_with_diagnostics(
            service,
            &key(&dir, "dead service"),
            "dead service",
            Some(&diagnostics),
        )
        .send(retain());
        diagnostics.emit();
    });
    assert!(service.registry().entries.is_empty());
    assert!(service.registry().deferred.is_empty());
    let log = output(&buffer);
    let (later, _receiver) = self::service();
    let path = verified_records(&snapshot).unwrap().pop().unwrap();
    later.enforce_retention(&snapshot, &path);
    assert_eq!(records(&dir).unwrap().len(), 16);
    fs::remove_dir_all(dir).unwrap();
    assert_eq!(
        log.matches("bus.draft.copy_recovery_disconnected").count(),
        1,
        "{log}"
    );
    assert!(
        log.contains("resolutions=2") && log.contains("deferrals=1"),
        "{log}"
    );
    assert!(!log.contains("private content"), "{log}");
}

#[test]
fn copy_recovery_round5_refused_latest_retain_waits_for_older_queued_save() {
    let dir = crate::utils::test_temp::unique_temp_path("copy-refused-latest");
    let text = "same draft in three attempts";
    let snapshot = key(&dir, text);
    let (service, receiver) = service();
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let dispatch = dispatch_for(&buffer);
    test_hooks::install(dir.clone(), || Err(io::Error::other("first save failed")));
    tracing::dispatcher::with_default(&dispatch, || {
        begin(service, &snapshot, text).send(retain());
    });
    service.step(receiver.recv().unwrap()); // Save A fails and queues Settle.
    begin(service, &snapshot, text).send(retain()); // Save B queues behind Settle.
    for index in 1..QUEUED_BACKUPS {
        let payload = format!("filler {index}");
        begin(service, &key(&dir.join("filler"), &payload), &payload).send(Outcome::Restored);
    }
    tracing::dispatcher::with_default(&dispatch, || {
        begin(service, &snapshot, text).send(retain()); // C cannot enqueue a save.
    });

    drain(service, &receiver);
    let log = output(&buffer);
    let path = verified_records(&snapshot).unwrap().pop().unwrap();
    let bytes = fs::read_to_string(&path).unwrap();
    fs::remove_dir_all(dir).unwrap();
    assert_eq!(bytes, text);
    assert!(!log.contains("bus.draft.copy_loss"), "{log}");
    assert_eq!(
        log.matches("bus.draft.recovery_required").count(),
        1,
        "{log}"
    );
    assert!(log.contains(&*path.to_string_lossy()), "{log}");
}

#[test]
fn copy_recovery_round5_blocked_scan_keeps_pin_until_first_eligible_prune() {
    let dir = crate::utils::test_temp::unique_temp_path("copy-ordinary-blocked-prune");
    let pane = dir.join("pane");
    let (service, receiver) = service();
    let mut held = Vec::new();
    for index in 0..16 {
        let text = format!("protected record {index}");
        let snapshot = key(&pane, &text);
        make_record(&snapshot, &text);
        held.push(begin(service, &snapshot, &text));
    }
    held.extend(fill_tracking(service, &dir.join("held")));
    drain(service, &receiver);
    let text = "oldest overflow draft";
    let snapshot = key(&pane, text);
    let path = make_record(&snapshot, text);
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH)
        .unwrap();
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let dispatch = dispatch_for(&buffer);
    tracing::dispatcher::with_default(&dispatch, || {
        begin(service, &snapshot, text).send(retain());
    });
    drain(service, &receiver);
    assert_eq!(records(&pane).unwrap().len(), 17);
    let log = output(&buffer);
    assert!(
        log.contains("bus.draft.recovery_required") && log.contains(&*path.to_string_lossy()),
        "{log}"
    );

    let victim = make_record(&key(&pane, "unprotected backup"), "unprotected backup");
    let replacement_key = key(&pane, "replacement");
    let replacement = make_record(&replacement_key, "replacement");
    service.enforce_retention(&replacement_key, &replacement);
    let first_pass_kept_pin = path.exists() && !victim.exists();
    service.enforce_retention(&replacement_key, &replacement);
    let second_pass_evicted_pin = !path.exists();
    fs::remove_dir_all(dir).unwrap();
    assert!(
        first_pass_kept_pin,
        "the first eligible scan evicted the copy just reported"
    );
    assert!(
        second_pass_evicted_pin,
        "a pin survived after it protected its copy"
    );
    drop(held);
}

#[test]
fn copy_recovery_round5_pin_releases_without_an_unrelated_save() {
    let dir = crate::utils::test_temp::unique_temp_path("copy-idle-overflow-pin");
    let (service, receiver) = service();
    let held = fill_tracking(service, &dir.join("held"));
    assert_eq!(receiver.try_iter().count(), QUEUED_BACKUPS);
    let text = "verified overflow backup";
    let snapshot = key(&dir.join("pane"), text);
    let path = make_record(&snapshot, text);
    begin(service, &snapshot, text).send(retain());
    drain(service, &receiver);
    let pin_released = !service.registry().overflow.contains_key(&snapshot);
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
    fs::remove_dir_all(dir).unwrap();
    assert!(pin_released, "an idle pane still occupied an overflow slot");
    drop(held);
}

fn check_idle_overflow_pin_after_scan_fault(
    failures: usize,
    delete_directory: bool,
    scans: usize,
    release_reason: Option<&str>,
) {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let dir = crate::utils::test_temp::unique_temp_path("copy-idle-pin-scan-fault");
    let pane = dir.join("pane");
    let moved = dir.join("removed-pane");
    let (service, receiver) = service();
    let held = fill_tracking(service, &dir.join("held"));
    assert_eq!(receiver.try_iter().count(), QUEUED_BACKUPS);
    let text = "verified overflow backup";
    let snapshot = key(&pane, text);
    let path = make_record(&snapshot, text);
    let reads = Arc::new(AtomicUsize::new(0));
    let (hook_dir, hook_moved, hook_reads) = (pane.clone(), moved.clone(), reads.clone());
    // This callback runs after the file was verified and the first cap
    // check passed. Fault only the idle pin's cap check and any retries.
    test_hooks::install_effect(path.clone(), move || {
        if delete_directory {
            fs::rename(&hook_dir, hook_moved).unwrap();
        }
        test_hooks::install_read(hook_dir, move || {
            if hook_reads.fetch_add(1, Ordering::SeqCst) < failures {
                Err(io::Error::other("injected cap read failure"))
            } else {
                Ok(())
            }
        });
    });

    let buffer = Arc::new(Mutex::new(Vec::new()));
    let dispatch = dispatch_for(&buffer);
    tracing::dispatcher::with_default(&dispatch, || {
        begin(service, &snapshot, text).send(retain());
        drain(service, &receiver);
    });
    let pin_released = !service.registry().overflow.contains_key(&snapshot);
    let log = output(&buffer);
    let backup = if delete_directory {
        assert!(!pane.exists());
        moved.join(path.file_name().unwrap())
    } else {
        path
    };
    assert_eq!(fs::read_to_string(backup).unwrap(), text);
    test_hooks::remove_read(&pane);
    fs::remove_dir_all(dir).unwrap();
    assert_eq!(
        log.matches("bus.draft.recovery_required").count(),
        1,
        "{log}"
    );
    assert!(
        pin_released,
        "an idle overflow slot remained occupied after the scan fault: {log}"
    );
    assert_eq!(reads.load(Ordering::SeqCst), scans, "{log}");
    if let Some(reason) = release_reason {
        assert_eq!(log.matches(reason).count(), 1, "{log}");
    } else {
        assert!(!log.contains("snapshot_prune_failed"), "{log}");
    }
    drop(held);
}

#[test]
fn copy_recovery_round6_idle_pin_recovers_from_one_failed_scan() {
    check_idle_overflow_pin_after_scan_fault(1, false, 2, None);
}

#[test]
fn copy_recovery_round6_idle_pin_recovers_from_two_failed_scans() {
    check_idle_overflow_pin_after_scan_fault(2, false, 3, None);
}

#[test]
fn copy_recovery_round6_idle_pin_releases_after_retries_are_exhausted() {
    check_idle_overflow_pin_after_scan_fault(
        usize::MAX,
        false,
        3,
        Some("snapshot_prune_pin_retries_exhausted"),
    );
}

#[test]
fn copy_recovery_round6_idle_pin_releases_when_directory_disappears() {
    check_idle_overflow_pin_after_scan_fault(
        0,
        true,
        1,
        Some("snapshot_prune_pin_directory_missing"),
    );
}

fn check_failed_revisit_reports_both_released_pins(directory_missing: bool, reason: &str) {
    let dir = crate::utils::test_temp::unique_temp_path("copy-two-pin-diagnostics");
    let pane = dir.join("pane");
    let (service, receiver) = service();
    let held = fill_tracking(service, &dir.join("held"));
    assert_eq!(receiver.try_iter().count(), QUEUED_BACKUPS);
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let dispatch = dispatch_for(&buffer);
    let pins = ["first retained draft", "second retained draft"].map(|text| {
        let snapshot = key(&pane, text);
        let path = make_record(&snapshot, text);
        let attempt =
            tracing::dispatcher::with_default(&dispatch, || begin(service, &snapshot, text));
        (snapshot, path, attempt)
    });
    for i in 0..15 {
        let text = format!("older backup {i}");
        make_record(&key(&pane, &text), &text);
    }
    let overflow_held: Vec<_> = (0..MAX_OVERFLOW - pins.len())
        .map(|i| {
            let text = format!("other overflow slot {i}");
            begin(service, &key(&dir.join("held"), &text), &text)
        })
        .collect();
    let text = "blocks all cap eviction";
    let untracked = begin(service, &key(&dir.join("held"), text), text);

    let [(first, first_path, first_attempt), (second, second_path, second_attempt)] = pins;
    tracing::dispatcher::with_default(&dispatch, || {
        first_attempt.send(retain());
        second_attempt.send(retain());
        drain(service, &receiver);
    });
    {
        let registry = service.registry();
        for snapshot in [&first, &second] {
            assert!(registry.overflow[snapshot].pin_next_prune);
        }
        assert_eq!(registry.deferred.len(), 1);
    }

    let backups = if directory_missing {
        let moved = dir.join("removed-pane");
        fs::rename(&pane, &moved).unwrap();
        [first_path, second_path].map(|path| moved.join(path.file_name().unwrap()))
    } else {
        test_hooks::install_read(pane.clone(), || Err(io::Error::other("failed revisit")));
        [first_path, second_path]
    };
    tracing::dispatcher::with_default(&dispatch, || {
        untracked.send(Outcome::Restored);
        drain(service, &receiver);
    });
    if !directory_missing {
        test_hooks::remove_read(&pane);
    }
    for (path, text) in backups
        .into_iter()
        .zip(["first retained draft", "second retained draft"])
    {
        assert_eq!(fs::read_to_string(path).unwrap(), text);
    }
    {
        let registry = service.registry();
        assert!(!registry.overflow.contains_key(&first));
        assert!(!registry.overflow.contains_key(&second));
        assert!(registry.deferred.is_empty());
    }
    fs::remove_dir_all(dir).unwrap();
    let log = output(&buffer);
    assert_eq!(
        log.matches("bus.draft.recovery_required").count(),
        2,
        "{log}"
    );
    let releases: Vec<_> = log.lines().filter(|line| line.contains(reason)).collect();
    assert_eq!(releases.len(), 2, "{log}");
    for snapshot in [first, second] {
        let digest_field = format!("digest={}", snapshot.digest);
        assert_eq!(
            releases
                .iter()
                .filter(|line| line.contains(&digest_field))
                .count(),
            1,
            "{log}"
        );
    }
    drop((held, overflow_held));
}

#[test]
fn copy_recovery_round7_exhausted_revisit_records_both_released_pins() {
    check_failed_revisit_reports_both_released_pins(false, "snapshot_prune_pin_retries_exhausted");
}

#[test]
fn copy_recovery_round7_missing_directory_records_both_released_pins() {
    check_failed_revisit_reports_both_released_pins(true, "snapshot_prune_pin_directory_missing");
}
