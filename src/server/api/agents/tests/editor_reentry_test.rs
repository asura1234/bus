//! Real exchange-lock and callback faults at the native submission boundary.
use super::preserving_observer;
use crate::{
    agents::AgentKind,
    platform::draft_editor::{test_run_at, Exchange},
    terminal::{runtime::PromptNotShown, TerminalRuntime},
};
use bytes::Bytes;
use std::{
    fs::{self, File, OpenOptions},
    io,
    path::PathBuf,
    sync::mpsc::{Receiver, RecvTimeoutError},
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

const DRAFT: &str = "developer draft-1";
const OLD: &str = "request from interrupted attempt";
const NEW: &str = "next request";
const SHORT: Duration = Duration::from_millis(800);
const WAIT: Duration = Duration::from_secs(10);

struct Pane {
    runtime: TerminalRuntime,
    writes: mpsc::Receiver<Bytes>,
    root: PathBuf,
    directory: PathBuf,
    file: PathBuf,
    draft: String,
    submits: Vec<String>,
}

impl Pane {
    fn new() -> Self {
        let root = crate::utils::test_temp::unique_temp_path("editor-reentry");
        fs::create_dir_all(&root).unwrap();
        let (runtime, writes) = TerminalRuntime::test_with_channel_capacity(120, 30, 16);
        runtime.test_set_input_recovery_directory(root.join("held-input"));
        let pane = Self {
            runtime,
            writes,
            directory: root.join("exchange"),
            file: root.join("native.txt"),
            root,
            draft: DRAFT.into(),
            submits: Vec::new(),
        };
        pane.paint();
        pane
    }

    fn paint(&self) {
        self.runtime.test_process_pty_bytes(
            format!("\x1b[2J\x1b[H› {}\r\n\r\n GPT-6 · /repo", self.draft).as_bytes(),
        );
    }

    fn queue(&self, payload: &str, budget: Duration) -> Receiver<io::Result<()>> {
        self.runtime
            .queue_observed_input_with_inspector(
                Bytes::from(format!("\x1b[200~{payload}\x1b[201~")),
                Bytes::from_static(b"\r"),
                vec![Bytes::from_static(b"\x01"), Bytes::from_static(b"\x0b")],
                Duration::ZERO,
                Some(Instant::now() + budget),
                Box::new(preserving_observer(
                    AgentKind::Codex,
                    payload.into(),
                    self.directory.clone(),
                    vec![
                        Bytes::from_static(b"\x13"),
                        Bytes::from_static(b"\x07"),
                        Bytes::from_static(b"\x7f"),
                    ],
                )),
            )
            .unwrap()
            .into_receiver()
    }

    fn hold_callback_lock(&self) -> File {
        let guard = OpenOptions::new()
            .read(true)
            .write(true)
            .open(self.directory.join("exchange.lock"))
            .unwrap();
        guard.lock().unwrap();
        guard
    }

    async fn next_write(&mut self) -> Bytes {
        tokio::time::timeout(WAIT, self.writes.recv())
            .await
            .unwrap()
            .unwrap()
    }

    fn emulate(&mut self, chunk: Bytes) {
        let text = std::str::from_utf8(&chunk).unwrap();
        match text {
            "\x07" => {
                fs::write(&self.file, &self.draft).unwrap();
                test_run_at(&self.directory, &self.file).unwrap();
                self.draft = fs::read_to_string(&self.file).unwrap();
            }
            "\r" => self.submits.push(std::mem::take(&mut self.draft)),
            "\x01" => {}
            "\x0b" | "\x15" => self.draft.clear(),
            "-2" => self.draft.push_str(text),
            text if text.bytes().all(|byte| byte == 0x7f) => {
                let marker = self.draft.find("__BUS_DRAFT_END_").unwrap();
                assert_eq!(self.draft.len() - marker, text.len());
                self.draft.truncate(marker);
            }
            text => self.draft.push_str(
                text.strip_prefix("\x1b[200~")
                    .unwrap()
                    .strip_suffix("\x1b[201~")
                    .unwrap(),
            ),
        }
        self.paint();
    }

    async fn pump(&mut self, response: Receiver<io::Result<()>>) -> io::Result<()> {
        let mut response =
            tokio::task::spawn_blocking(move || response.recv_timeout(WAIT).unwrap());
        loop {
            tokio::select! {
                reply = &mut response => return reply.unwrap(),
                chunk = self.next_write() => self.emulate(chunk),
            }
        }
    }

    fn expire_pending_swap(&self) {
        let request = self.directory.join("request.json");
        let mut body: serde_json::Value =
            serde_json::from_slice(&fs::read(&request).unwrap()).unwrap();
        body["expires_ms"] = 0.into();
        fs::write(request, serde_json::to_vec(&body).unwrap()).unwrap();
    }
}

impl Drop for Pane {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn composer_busy(result: io::Result<()>) {
    assert!(PromptNotShown::composer_busy(&result.unwrap_err()));
}

async fn contended_reentry(saved: bool) {
    let mut pane = Pane::new();
    let log = pane.root.join("editor-events.log");
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(File::create(&log).unwrap())
        .finish();
    let first = tracing::subscriber::with_default(subscriber, || pane.queue(OLD, SHORT));
    assert_eq!(pane.next_write().await.as_ref(), b"\x07");
    let recovery = if saved {
        fs::write(&pane.file, DRAFT).unwrap();
        test_run_at(&pane.directory, &pane.file).unwrap();
        let record = pane.directory.join("preserved.json");
        let bytes = fs::read(&record).unwrap();
        let token = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["token"]
            .as_str()
            .unwrap()
            .to_owned();
        Some((
            record,
            bytes,
            pane.directory.join(format!("preserved-{token}.txt")),
        ))
    } else {
        None
    };
    // Hold the same OS lock the native callback owns. Delay its render so the
    // first attempt reaches its deadline even when a backup is already durable.
    let callback = pane.hold_callback_lock();
    if saved {
        // Crash after publishing the backup, ahead of the acknowledgement.
        fs::remove_file(pane.directory.join("ack.json")).unwrap();
    }
    composer_busy(first.recv_timeout(WAIT).unwrap());
    if saved {
        assert!(fs::read_to_string(log)
            .unwrap()
            .contains("bus.draft.recovery_required"));
        pane.draft.clear();
        pane.paint();
    }
    let retry_log = pane.root.join("retry-events.log");
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(File::create(&retry_log).unwrap())
        .finish();
    let reentry = tracing::subscriber::with_default(subscriber, || pane.queue(NEW, SHORT));
    if saved {
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            reentry.try_recv().is_err(),
            "restoration must honor the wait budget"
        );
        pane.runtime
            .try_send_developer_bytes(Bytes::from_static(b"-2"))
            .unwrap();
    }
    let refused = reentry.recv_timeout(SHORT + Duration::from_millis(500));
    if matches!(refused, Err(RecvTimeoutError::Timeout)) {
        drop(callback);
        panic!("re-entry held the pane lease past its deadline while the callback held the lock");
    }
    composer_busy(refused.unwrap());
    assert!(
        pane.writes.try_recv().is_err(),
        "refusal writes no native keys"
    );
    pane.runtime
        .try_send_developer_bytes(Bytes::from_static(b"\x0c"))
        .unwrap();
    assert_eq!(
        pane.next_write().await.as_ref(),
        b"\x0c",
        "the pane lease was released"
    );
    drop(callback);

    if saved {
        let path = fs::read_to_string(retry_log)
            .unwrap()
            .lines()
            .find(|line| line.contains("bus.draft.input_recovery_required"))
            .and_then(|line| line.split_once("recovery="))
            .map(|(_, path)| PathBuf::from(path.trim()))
            .expect("the refused restore records the buffered input's recovery file");
        assert_eq!(fs::read(&path).unwrap(), b"-2");
        fs::remove_file(path).unwrap();
    }
    if let Some((record, bytes, backup)) = recovery {
        assert_eq!(fs::read(&record).unwrap(), bytes);
        assert_eq!(fs::read_to_string(&backup).unwrap(), DRAFT);
        let repair = pane.queue(NEW, WAIT);
        composer_busy(pane.pump(repair).await);
        assert_eq!(pane.draft, DRAFT);
        assert!(!record.exists());
    } else {
        pane.expire_pending_swap();
    }
    let send = pane.queue(NEW, WAIT);
    pane.pump(send).await.unwrap();
    assert_eq!(pane.submits, [NEW]);
    assert_eq!(pane.draft, DRAFT);
}

#[tokio::test]
async fn editor_reentry_contended_swap_refuses_within_deadline() {
    contended_reentry(false).await;
}

#[tokio::test]
async fn editor_reentry_contended_restore_refuses_and_keeps_backup() {
    contended_reentry(true).await;
}

#[tokio::test]
async fn editor_reentry_expired_uncommitted_swap_allows_copy_and_fences_late_callback() {
    let mut pane = Pane::new();
    let mut exchange = Exchange::begin_preserving(pane.directory.clone(), WAIT).unwrap();
    exchange.request_swap(OLD).unwrap();
    exchange.defer_cleanup();
    drop(exchange);
    pane.expire_pending_swap();

    let send = pane.queue(NEW, WAIT);
    let mut response = tokio::task::spawn_blocking(move || send.recv_timeout(WAIT).unwrap());
    let first = tokio::select! {
        chunk = pane.next_write() => chunk,
        reply = &mut response => panic!("expired uncommitted swap vetoed delivery: {:?}", reply.unwrap()),
    };
    assert_eq!(
        first.as_ref(),
        b"\x01",
        "copy starts without launching another editor"
    );
    fs::write(&pane.file, DRAFT).unwrap();
    assert!(test_run_at(&pane.directory, &pane.file).is_err());
    assert_eq!(fs::read_to_string(&pane.file).unwrap(), DRAFT);
    pane.emulate(first);
    loop {
        tokio::select! {
            reply = &mut response => {
                reply.unwrap().unwrap();
                break;
            }
            chunk = pane.next_write() => pane.emulate(chunk),
        }
    }
    assert_eq!(pane.submits, [NEW]);
    assert_eq!(pane.draft, DRAFT);
}

#[tokio::test]
async fn editor_reentry_within_budget_restores_and_replays_developer_edit() {
    let mut pane = Pane::new();
    fs::write(&pane.file, DRAFT).unwrap();
    let original = Exchange::begin_preserving(pane.directory.clone(), WAIT).unwrap();
    original.request_swap(OLD).unwrap();
    test_run_at(&pane.directory, &pane.file).unwrap();
    drop(original);
    pane.draft.clear();
    pane.paint();

    let callback = pane.hold_callback_lock();
    let reply = pane.queue(NEW, WAIT);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let early = reply.try_recv();
    if early.is_ok() {
        drop(callback);
        panic!("restoration refused a callback that was still within its deadline");
    }
    pane.runtime
        .try_send_developer_bytes(Bytes::from_static(b"-2"))
        .unwrap();
    tokio::time::sleep(Duration::from_millis(458)).await;
    assert!(
        pane.writes.try_recv().is_err(),
        "the edit must wait for the native draft"
    );
    drop(callback);
    composer_busy(pane.pump(reply).await);
    while let Ok(key) = pane.writes.try_recv() {
        pane.emulate(key);
    }
    assert_eq!(pane.draft, "developer draft-1-2");
    assert!(!pane.draft.contains("__BUS_DRAFT_END_") && pane.submits.is_empty());
}

#[tokio::test]
async fn editor_reentry_stalled_fence_sync_never_authorizes_fallback() {
    let mut pane = Pane::new();
    let mut exchange = Exchange::begin_preserving(pane.directory.clone(), WAIT).unwrap();
    exchange.request_swap(OLD).unwrap();
    exchange.defer_cleanup();
    drop(exchange);
    pane.expire_pending_swap();

    let (sync_started, resume) =
        crate::platform::draft_editor::fence_test_hooks::stall(pane.directory.clone());
    let reply = pane.queue(NEW, SHORT);
    sync_started.recv_timeout(WAIT).unwrap();
    assert!(
        pane.writes.try_recv().is_err(),
        "no copy while the fence is only renamed"
    );
    fs::write(&pane.file, DRAFT).unwrap();
    let directory = pane.directory.clone();
    let file = pane.file.clone();
    let (done, callback) = std::sync::mpsc::channel();
    std::thread::spawn(move || done.send(test_run_at(&directory, &file)).unwrap());
    let result = reply.recv_timeout(SHORT + Duration::from_millis(400));
    let waiting = callback.try_recv().is_err();
    assert!(
        pane.writes.try_recv().is_err(),
        "no native input during publication"
    );
    resume.send(()).unwrap();
    assert!(waiting && callback.recv_timeout(WAIT).unwrap().is_err());
    assert_eq!(fs::read_to_string(&pane.file).unwrap(), DRAFT);
    composer_busy(result.expect("a stalled directory sync held the pane past its deadline"));

    let retry = pane.queue(NEW, WAIT);
    pane.pump(retry).await.unwrap();
    assert_eq!(pane.submits, [NEW]);
    assert_eq!(pane.draft, DRAFT);
}
