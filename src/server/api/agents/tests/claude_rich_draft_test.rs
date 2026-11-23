//! Native paste receipts must not submit the previous empty Claude frame.
use super::*;
use crate::terminal::runtime::TerminalRuntime;
use std::time::Duration;

const ORIGINAL: &str = "[Image #1][Pasted text #2 +60 lines]";
const PASTED: &str = "[Image #3][Pasted text #4 +34 lines]";

fn paint(runtime: &TerminalRuntime, body: &str, stashed: bool, activity: &str) {
    let stash = if stashed { "› stashed" } else { "" };
    let frame = format!("{activity}\n{stash}\n────────────────\n❯ {body}\n────────────────");
    runtime
        .test_process_pty_bytes(format!("\x1b[2J\x1b[H{}", frame.replace('\n', "\r\n")).as_bytes());
}

async fn key(writes: &mut tokio::sync::mpsc::Receiver<Bytes>) -> Bytes {
    tokio::time::timeout(Duration::from_secs(5), writes.recv())
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn claude_plain_paste_held_frame_cannot_submit_a_restored_native_draft() {
    let mut retries = Vec::new();
    for stash in ["empty", "captured", "existing"] {
        let directory = crate::utils::test_temp::unique_temp_path("claude-held-plain-frame");
        let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
        let clock = runtime.test_submission_clock().unwrap();
        clock.pause();
        let original = "developer draft";
        paint(
            &runtime,
            if stash == "captured" { original } else { "" },
            stash == "existing",
            "initial",
        );
        let completion = runtime
            .queue_observed_input_with_inspector(
                Bytes::from_static(b"from bus"),
                Bytes::from_static(b"\r"),
                vec![
                    Bytes::from_static(b"\x01"),
                    Bytes::from_static(b"\x0b"),
                    Bytes::from_static(b"\x15"),
                ],
                Duration::ZERO,
                None,
                Box::new(PreservingComposer::new(
                    AgentKind::Claude,
                    "from bus".into(),
                    directory.clone(),
                    vec![
                        Bytes::from_static(b"\x13"),
                        Bytes::from_static(b"\x07"),
                        Bytes::from_static(b"\x7f"),
                    ],
                )),
            )
            .unwrap();
        if stash == "captured" {
            assert_eq!(key(&mut writes).await.as_ref(), b"\x13");
            paint(&runtime, "", true, "native draft stashed");
        }
        assert_eq!(key(&mut writes).await.as_ref(), b"from bus");
        paint(&runtime, "from bus", stash != "empty", "Bus paste rendered");
        assert_eq!(key(&mut writes).await.as_ref(), b"\r");
        clock.wait_for_enters(1);
        // Claude consumed Enter, restoring a developer stash when present,
        // but its repaint is held outside Bus. The parsed frame still contains Bus.
        let generation = clock.advance(Duration::from_millis(300));
        clock.wait_for_submission_check(generation);
        let stale_retry = writes.try_recv().ok();
        // Even a later repaint retaining Bus must not permit a stash retry.
        let mut stash_retry = None;
        if stash != "empty" && stale_retry.is_none() {
            paint(&runtime, "from bus", true, "unrelated repaint");
            let generation = clock.advance(Duration::from_millis(300));
            clock.wait_for_submission_check(generation);
            stash_retry = writes.try_recv().ok();
        }
        paint(
            &runtime,
            if stash == "empty" { "" } else { original },
            false,
            "consumption repaint released",
        );
        completion
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        if directory.exists() {
            std::fs::remove_dir_all(directory).unwrap();
        }
        if stale_retry.is_some() || stash_retry.is_some() {
            retries.push((stash, stale_retry, stash_retry));
        }
        assert!(writes.try_recv().is_err());
    }
    assert!(
        retries.is_empty(),
        "held/native-stash frames authorized another Enter: {retries:?}"
    );
}

async fn delayed_paste(painted_before_receipt: bool, unrelated_output: bool) {
    let directory = crate::utils::test_temp::unique_temp_path("claude-delayed-rich-paste");
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    let clock = runtime.test_submission_clock().unwrap();
    clock.pause();
    paint(&runtime, ORIGINAL, false, "original draft");
    // This is the real large-paste shape from the native RED: Claude lifts
    // the inline unquoted PNG while collapsing the rest into one text chip.
    let payload = format!(
        "Reply with only STASH_MARKER. Do not use tools.\n\n{}\nDialog screenshot: /private/tmp/native-rich/fixture.png",
        (0..32)
            .map(|i| format!("Preserved report detail {i}: native delivery must bind the correct turn."))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let inspector = PreservingComposer::new(
        AgentKind::Claude,
        payload.clone(),
        directory.clone(),
        vec![
            Bytes::from_static(b"\x13"),
            Bytes::from_static(b"\x07"),
            Bytes::from_static(b"\x7f"),
        ],
    );
    let completion = runtime
        .queue_observed_input_with_inspector(
            Bytes::from(payload.clone()),
            Bytes::from_static(b"\r"),
            vec![
                Bytes::from_static(b"\x01"),
                Bytes::from_static(b"\x0b"),
                Bytes::from_static(b"\x15"),
            ],
            Duration::ZERO,
            None,
            Box::new(inspector),
        )
        .unwrap();
    assert_eq!(key(&mut writes).await.as_ref(), b"\x13");
    let paste_receipt = clock.hold_next_write_completion();
    paint(&runtime, "", true, "draft stashed");
    assert_eq!(key(&mut writes).await.as_ref(), payload.as_bytes());
    runtime
        .try_send_developer_bytes(Bytes::from_static(b"typed meanwhile"))
        .unwrap();
    if painted_before_receipt {
        paint(&runtime, PASTED, true, "native paste finished");
    } else if unrelated_output {
        // Transcript/OSC output advances native content without materializing
        // any input. A sequence change alone cannot authorize Enter.
        paint(&runtime, "", true, "unrelated transcript update");
        runtime.test_process_pty_bytes(b"\x1b]0;unrelated-title\x07");
    }
    paste_receipt.send(()).unwrap();
    let waited_for_input = clock.wait_for_draft_wait_or_enter();
    if !painted_before_receipt && waited_for_input {
        paint(&runtime, PASTED, true, "native paste finished");
    }
    assert_eq!(key(&mut writes).await.as_ref(), b"\r");
    // Complete native restoration even on base, so the failing ordering
    // assertion leaves no writer thread or clock waiter behind.
    paint(&runtime, ORIGINAL, false, "original draft returned");
    completion
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert_eq!(key(&mut writes).await.as_ref(), b"typed meanwhile");
    assert!(runtime.visible_ansi().contains(ORIGINAL));
    assert!(writes.try_recv().is_err(), "one Enter, no duplicate paste");
    std::fs::remove_dir_all(directory).unwrap();
    assert!(
        painted_before_receipt || waited_for_input,
        "Claude received Enter against the empty stashed frame before its paste materialized"
    );
}

#[tokio::test]
async fn claude_rich_paste_waits_for_materialized_input_after_write_receipt() {
    delayed_paste(false, false).await;
}

#[tokio::test]
async fn claude_rich_paste_unrelated_output_is_not_input_readiness() {
    delayed_paste(false, true).await;
}

#[tokio::test]
async fn claude_rich_paste_render_before_flush_receipt_is_not_missed() {
    delayed_paste(true, false).await;
}

#[tokio::test]
async fn claude_rich_paste_timeout_does_not_restore_over_an_unresolved_flush() {
    let directory = crate::utils::test_temp::unique_temp_path("claude-late-rich-paste");
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    let clock = runtime.test_submission_clock().unwrap();
    clock.pause();
    paint(&runtime, ORIGINAL, false, "original draft");
    let payload = "Long report\nDialog screenshot: /private/tmp/native-rich/fixture.png";
    let inspector = PreservingComposer::new(
        AgentKind::Claude,
        payload.into(),
        directory.clone(),
        vec![
            Bytes::from_static(b"\x13"),
            Bytes::from_static(b"\x07"),
            Bytes::from_static(b"\x7f"),
        ],
    );
    let completion = runtime
        .queue_observed_input_with_inspector(
            Bytes::from_static(payload.as_bytes()),
            Bytes::from_static(b"\r"),
            vec![Bytes::from_static(b"\x15")],
            Duration::ZERO,
            None,
            Box::new(inspector),
        )
        .unwrap();
    assert_eq!(key(&mut writes).await.as_ref(), b"\x13");
    paint(&runtime, "", true, "draft stashed");
    assert_eq!(key(&mut writes).await.as_ref(), payload.as_bytes());
    assert!(clock.wait_for_draft_wait_or_enter());
    clock.advance(Duration::from_secs(3));
    let mut response = tokio::task::spawn_blocking(move || {
        completion.recv_timeout(Duration::from_secs(5)).unwrap()
    });
    let restored_before_resolving_paste;
    let result;
    tokio::select! {
        bytes = writes.recv() => {
            assert_eq!(bytes.unwrap().as_ref(), b"\x13");
            // Finish the old fallback before failing the assertion, leaving
            // no submission worker or paused clock waiting behind the probe.
            paint(&runtime, ORIGINAL, false, "premature original restoration");
            result = response.await.unwrap();
            restored_before_resolving_paste = true;
        }
        completed = &mut response => {
            result = completed.unwrap();
            restored_before_resolving_paste = false;
        }
    }
    assert!(result.is_err());
    // The native reader may still apply the flushed payload after timeout.
    paint(&runtime, payload, true, "late native paste");
    let retained = std::fs::read_dir(&directory)
        .unwrap()
        .flatten()
        .any(|entry| entry.file_name().to_string_lossy().starts_with("draft-"));
    std::fs::remove_dir_all(directory).unwrap();
    assert!(
        !restored_before_resolving_paste,
        "stash restoration overtook a successfully flushed but unresolved paste"
    );
    assert!(
        retained,
        "retain recovery until the native draft is proven restored"
    );
    assert!(
        writes.try_recv().is_err(),
        "no Enter or replay after timeout"
    );
}

#[tokio::test]
async fn claude_existing_stash_waits_for_rich_paste_materialization() {
    struct ReadinessTrace {
        inner: PreservingComposer,
        sampled: std::sync::mpsc::Sender<bool>,
    }
    impl SubmissionInspector for ReadinessTrace {
        fn observe(&self, screen: &str) -> InputObservation {
            self.inner.observe(screen)
        }
        fn observe_with_launch(
            &self,
            screen: &str,
            cursor: Option<(u16, u16)>,
            pristine: bool,
        ) -> InputObservation {
            self.inner.observe_with_launch(screen, cursor, pristine)
        }
        fn submission_available(&self, screen: &str) -> bool {
            self.inner.submission_available(screen)
        }
        fn paste_ready(&self, screen: &str) -> bool {
            let ready = self.inner.paste_ready(screen);
            let _ = self.sampled.send(ready);
            ready
        }
        fn preservation_budget(&self) -> Duration {
            self.inner.preservation_budget()
        }
        fn draft_preservation_since_launch(
            &self,
            screen: &str,
            cursor: Option<(u16, u16)>,
            pristine: bool,
        ) -> Option<DraftPreservation> {
            self.inner
                .draft_preservation_since_launch(screen, cursor, pristine)
        }
        fn native_stash_visible(&self, screen: &str) -> bool {
            self.inner.native_stash_visible(screen)
        }
        fn native_draft_restored(&self, screen: &str) -> bool {
            self.inner.native_draft_restored(screen)
        }
    }
    let directory = crate::utils::test_temp::unique_temp_path("claude-retained-stash-retry");
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    let clock = runtime.test_submission_clock().unwrap();
    clock.pause();
    let inspector = |expected: String| {
        PreservingComposer::new(
            AgentKind::Claude,
            expected,
            directory.clone(),
            vec![
                Bytes::from_static(b"\x13"),
                Bytes::from_static(b"\x07"),
                Bytes::from_static(b"\x7f"),
            ],
        )
    };
    let old_payload = "Late report\nDialog screenshot: /private/tmp/native-rich/fixture.png";
    paint(&runtime, ORIGINAL, false, "original rich developer draft");
    let first = runtime
        .queue_observed_input_with_inspector(
            Bytes::from_static(old_payload.as_bytes()),
            Bytes::from_static(b"\r"),
            vec![Bytes::from_static(b"\x15")],
            Duration::ZERO,
            None,
            Box::new(inspector(old_payload.into())),
        )
        .unwrap();
    assert_eq!(key(&mut writes).await.as_ref(), b"\x13");
    paint(&runtime, "", true, "original draft stashed");
    assert_eq!(key(&mut writes).await.as_ref(), old_payload.as_bytes());
    assert!(clock.wait_for_draft_wait_or_enter());
    clock.advance(Duration::from_secs(3));
    assert!(first.recv_timeout(Duration::from_secs(5)).unwrap().is_err());
    assert!(
        writes.try_recv().is_err(),
        "timeout must retain stash without Enter"
    );
    // A later submission first recovers and withdraws the late, owned paste.
    paint(
        &runtime,
        old_payload,
        true,
        "late native paste materialized",
    );
    let payload = format!(
        "Reply with only STASH_MARKER.\n{}\nDialog screenshot: /private/tmp/native-rich/fixture.png",
        (0..32).map(|i| format!("Report detail {i}: keep the original developer draft."))
            .collect::<Vec<_>>().join("\n")
    );
    let (sampled, samples) = std::sync::mpsc::channel();
    let second = runtime
        .queue_observed_input_with_inspector(
            Bytes::from(payload.clone()),
            Bytes::from_static(b"\r"),
            vec![Bytes::from_static(b"\x15")],
            Duration::ZERO,
            None,
            Box::new(ReadinessTrace {
                inner: inspector(payload.clone()),
                sampled,
            }),
        )
        .unwrap();
    assert_eq!(key(&mut writes).await.as_ref(), b"\x15");
    paint(
        &runtime,
        "",
        true,
        "late Bus paste withdrawn, original stash retained",
    );
    assert_eq!(key(&mut writes).await.as_ref(), payload.as_bytes());
    // Observe the real submission inspector's first readiness decision while
    // the new paste has not rendered, avoiding a wall-clock ordering oracle.
    let ready_while_empty = samples.recv_timeout(Duration::from_secs(5)).unwrap();
    if !ready_while_empty {
        paint(&runtime, PASTED, true, "new rich paste materialized");
    }
    assert_eq!(key(&mut writes).await.as_ref(), b"\r");
    paint(&runtime, ORIGINAL, false, "original draft returned");
    second
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert!(runtime.visible_ansi().contains(ORIGINAL));
    assert!(
        writes.try_recv().is_err(),
        "one Enter, no stash toggle or duplicate paste"
    );
    std::fs::remove_dir_all(directory).unwrap();
    assert!(
        !ready_while_empty,
        "retained-stash recovery bypassed rich-paste readiness and Enter reached the empty frame"
    );
}

const COPIED_DRAFT: &str = "developer draft";
const INCOMING: &str = "from bus";

fn queue_claude_copy(
    runtime: &TerminalRuntime,
    directory: std::path::PathBuf,
    draft: &str,
) -> crate::terminal::pty::actor::InputCompletion {
    paint(runtime, draft, false, "");
    runtime
        .queue_observed_input_with_inspector(
            Bytes::from_static(INCOMING.as_bytes()),
            Bytes::from_static(b"\r"),
            vec![Bytes::from_static(b"\x15")],
            Duration::ZERO,
            None,
            Box::new(PreservingComposer::new(
                AgentKind::Claude,
                INCOMING.into(),
                directory,
                Vec::new(),
            )),
        )
        .unwrap()
}

#[tokio::test]
async fn claude_copy_fallback_retries_delayed_clear_before_submitting_and_restores() {
    let directory = crate::utils::test_temp::unique_temp_path("claude-copy-clear-retry");
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    let clock = runtime.test_submission_clock().unwrap();
    clock.pause();
    let (completion, events) = captured_copy(&runtime, directory.clone());

    // Ignore the first two clears. A paste here would append to the draft.
    for _ in 0..2 {
        assert_eq!(key(&mut writes).await.as_ref(), b"\x15");
        clock.advance(Duration::from_millis(25));
    }
    let third = key(&mut writes).await;
    if third.as_ref() != b"\x15" {
        clock.resume();
    }
    assert_eq!(
        third.as_ref(),
        b"\x15",
        "paste overtook the confirmed clear"
    );

    let mut composer = String::new();
    paint(&runtime, &composer, false, "");
    let pasted = key(&mut writes).await;
    composer.push_str(std::str::from_utf8(&pasted).unwrap());
    paint(&runtime, &composer, false, "");
    assert_eq!(key(&mut writes).await.as_ref(), b"\r");
    let submitted = std::mem::take(&mut composer);
    assert_eq!(submitted, INCOMING);
    clock.wait_for_enters(1);

    runtime.test_process_pty_bytes(
        "\x1b[2J\x1b[H> from bus\r\n✻ Working… (0s · esc to interrupt)\r\n".as_bytes(),
    );
    let generation = clock.advance(Duration::from_millis(100));
    clock.wait_for_submission_check(generation);
    assert!(
        writes.try_recv().is_err(),
        "restore must wait for an editable composer"
    );
    paint(&runtime, &composer, false, "");
    assert_eq!(
        key(&mut writes).await.as_ref(),
        b"\x1b[200~developer draft\x1b[201~"
    );
    paint(&runtime, COPIED_DRAFT, false, "");
    completion
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert!(writes.try_recv().is_err());
    copy_event(&events, "bus.draft.copy_recovery_retired");
    if directory.exists() {
        assert!(std::fs::read_dir(&directory).unwrap().next().is_none());
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[tokio::test]
async fn claude_copy_fallback_retains_draft_when_restore_lease_is_invalidated() {
    let directory = crate::utils::test_temp::unique_temp_path("claude-copy-restore-revoked");
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    let clock = runtime.test_submission_clock().unwrap();
    clock.pause();
    let (completion, events) = captured_copy(&runtime, directory.clone());
    assert_eq!(key(&mut writes).await.as_ref(), b"\x15");
    paint(&runtime, "", false, "");
    assert_eq!(key(&mut writes).await.as_ref(), INCOMING.as_bytes());
    paint(&runtime, INCOMING, false, "");
    assert_eq!(key(&mut writes).await.as_ref(), b"\r");
    clock.wait_for_enters(1);
    runtime.invalidate_submission_ownership();
    paint(&runtime, "", false, "");
    completion
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert!(
        writes.try_recv().is_err(),
        "revoked restore must never write"
    );
    copy_event(&events, "copy_restore_failed");
    let files: Vec<_> = std::fs::read_dir(&directory)
        .expect("keep the copied draft recoverable")
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1, "keep exactly one recovery record");
    assert_eq!(std::fs::read_to_string(&files[0]).unwrap(), COPIED_DRAFT);
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn claude_copy_fallback_expires_without_paste_when_clear_never_confirms() {
    let directory = crate::utils::test_temp::unique_temp_path("claude-copy-clear-expired");
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    let clock = runtime.test_submission_clock().unwrap();
    clock.pause();
    let receipt = clock.hold_next_write_completion();
    let (completion, events) = captured_copy(&runtime, directory.clone());
    assert_eq!(key(&mut writes).await.as_ref(), b"\x15");
    clock.advance(Duration::from_secs(2));
    receipt.send(()).unwrap();
    let mut result = tokio::task::spawn_blocking(move || {
        completion.recv_timeout(Duration::from_secs(5)).unwrap()
    });
    let error = tokio::select! {
        result = &mut result => result.unwrap().unwrap_err(),
        bytes = key(&mut writes) => {
            clock.resume();
            panic!("clear expiry wrote unexpected input: {bytes:?}");
        }
    };
    assert!(matches!(
        crate::terminal::runtime::PromptNotShown::classified(&error),
        Some(crate::terminal::runtime::PromptNotShown::NotSubmitted)
    ));
    assert!(writes.try_recv().is_err());
    assert!(runtime.visible_ansi().contains(COPIED_DRAFT));

    copy_event(&events, "clear_incomplete");

    // The queued retry sees the original draft again and clears it this
    // time. Its verified restore also retires the earlier recovery copy.
    let (retry, retry_events) = captured_copy(&runtime, directory.clone());
    accept_copy_message(&runtime, &mut writes).await;
    paint(&runtime, "", false, "");
    assert_eq!(
        key(&mut writes).await.as_ref(),
        b"\x1b[200~developer draft\x1b[201~"
    );
    paint(&runtime, COPIED_DRAFT, false, "");
    retry.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
    copy_event(&retry_events, "bus.draft.copy_recovery_retired");
    assert!(std::fs::read_dir(&directory).unwrap().next().is_none());
    assert!(writes.try_recv().is_err());
    std::fs::remove_dir_all(directory).unwrap();
}

struct CopyLogWriter {
    messages: std::sync::mpsc::Sender<String>,
    line: Vec<u8>,
}

impl std::io::Write for CopyLogWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.line.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for CopyLogWriter {
    fn drop(&mut self) {
        let line = String::from_utf8_lossy(&self.line);
        if line.contains("bus.draft.") {
            let _ = self.messages.send(line.into_owned());
        }
    }
}

fn captured_copy(
    runtime: &TerminalRuntime,
    directory: std::path::PathBuf,
) -> (
    crate::terminal::pty::actor::InputCompletion,
    std::sync::mpsc::Receiver<String>,
) {
    captured_copy_with_draft(runtime, directory, COPIED_DRAFT)
}

fn captured_copy_with_draft(
    runtime: &TerminalRuntime,
    directory: std::path::PathBuf,
    draft: &str,
) -> (
    crate::terminal::pty::actor::InputCompletion,
    std::sync::mpsc::Receiver<String>,
) {
    let (messages, events) = std::sync::mpsc::channel();
    let dispatch = tracing::Dispatch::new(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .without_time()
            .with_writer(move || CopyLogWriter {
                messages: messages.clone(),
                line: Vec::new(),
            })
            .finish(),
    );
    let completion = tracing::dispatcher::with_default(&dispatch, || {
        queue_claude_copy(runtime, directory, draft)
    });
    (completion, events)
}

fn copy_event(events: &std::sync::mpsc::Receiver<String>, name: &str) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while let Ok(line) =
        events.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
    {
        if line.contains(name) {
            return line;
        }
    }
    panic!("missing copy recovery event {name}");
}

async fn accept_copy_message(
    runtime: &TerminalRuntime,
    writes: &mut tokio::sync::mpsc::Receiver<Bytes>,
) {
    assert_eq!(key(writes).await.as_ref(), b"\x15");
    paint(runtime, "", false, "");
    assert_eq!(key(writes).await.as_ref(), INCOMING.as_bytes());
    paint(runtime, INCOMING, false, "");
    assert_eq!(key(writes).await.as_ref(), b"\r");
    runtime.test_submission_clock().unwrap().wait_for_enters(1);
}

fn copy_record(directory: &std::path::Path, text: &str) -> std::path::PathBuf {
    directory.join(format!(
        "copied-{}.txt",
        crate::utils::time::digest(text.as_bytes())
    ))
}

fn write_private_copy(path: &std::path::Path, text: &str) {
    use std::io::Write;
    crate::platform::create_private_state_file(path)
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
}

#[tokio::test]
async fn claude_copy_fallback_storage_failure_still_delivers_and_restores_from_ram() {
    let directory = crate::utils::test_temp::unique_temp_path("claude-copy-storage-fault");
    // A regular file as the directory yields a deterministic storage failure,
    // even under a privileged test runner.
    std::fs::write(&directory, "blocks recovery storage").unwrap();
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    runtime.test_submission_clock().unwrap().pause();
    let (completion, events) = captured_copy(&runtime, directory.clone());
    let mut result =
        tokio::task::spawn_blocking(move || completion.recv_timeout(Duration::from_secs(5)));
    tokio::select! {
        () = accept_copy_message(&runtime, &mut writes) => {}
        done = &mut result => panic!("storage failure refused an editable pane: {:?}", done.unwrap()),
    }
    paint(&runtime, "", false, "");
    assert_eq!(
        key(&mut writes).await.as_ref(),
        b"\x1b[200~developer draft\x1b[201~"
    );
    paint(&runtime, COPIED_DRAFT, false, "");
    result.await.unwrap().unwrap().unwrap();
    assert!(writes.try_recv().is_err());
    let event = copy_event(&events, "bus.draft.copy_recovery_failed");
    assert!(event.contains("snapshot_storage_failed"), "{event}");
    std::fs::remove_file(directory).unwrap();
}

#[tokio::test]
async fn claude_copy_fallback_replaces_unverified_record_with_verified_alternate() {
    for is_directory in [false, true] {
        let directory = crate::utils::test_temp::unique_temp_path("claude-copy-conflicting-record");
        crate::platform::fs::private_dir(&directory).unwrap();
        let conflicting = copy_record(&directory, COPIED_DRAFT);
        if is_directory {
            std::fs::create_dir(&conflicting).unwrap();
        } else {
            write_private_copy(&conflicting, "wrong bytes");
        }

        let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
        runtime.test_submission_clock().unwrap().pause();
        let (completion, events) = captured_copy(&runtime, directory.clone());
        accept_copy_message(&runtime, &mut writes).await;
        runtime.invalidate_submission_ownership();
        paint(&runtime, "", false, "");
        completion
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();

        let event = copy_event(&events, "bus.draft.recovery_required");
        let records: Vec<_> = std::fs::read_dir(&directory)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path != &conflicting)
            .collect();
        assert_eq!(records.len(), 1, "no fresh recovery record for {event}");
        assert_eq!(std::fs::read_to_string(&records[0]).unwrap(), COPIED_DRAFT);
        assert!(event.contains(&*records[0].to_string_lossy()), "{event}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&records[0]).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert!(writes.try_recv().is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[tokio::test]
async fn claude_copy_fallback_bounds_recovery_records_and_retires_a_reused_snapshot() {
    // Small records exercise the count cap and large ones exercise the byte cap.
    for record_bytes in [1024, 600 * 1024] {
        let directory = crate::utils::test_temp::unique_temp_path("claude-copy-bounded-records");
        crate::platform::fs::private_dir(&directory).unwrap();
        for index in 0..20 {
            let old = format!("{index:02}{}", "x".repeat(record_bytes));
            write_private_copy(&copy_record(&directory, &old), &old);
        }
        let snapshot = copy_record(&directory, COPIED_DRAFT);
        write_private_copy(&snapshot, COPIED_DRAFT);

        let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
        runtime.test_submission_clock().unwrap().pause();
        let (completion, events) = captured_copy(&runtime, directory.clone());
        accept_copy_message(&runtime, &mut writes).await;
        paint(&runtime, "", false, "");
        assert_eq!(
            key(&mut writes).await.as_ref(),
            b"\x1b[200~developer draft\x1b[201~"
        );
        paint(&runtime, COPIED_DRAFT, false, "");
        completion
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        // On the old implementation storage retirement is synchronous, so the
        // captured worker disconnects and these assertions see its settled state.
        while events.recv_timeout(Duration::from_secs(5)).is_ok() {}

        let records: Vec<_> = std::fs::read_dir(&directory).unwrap().flatten().collect();
        let bytes: u64 = records
            .iter()
            .map(|entry| entry.metadata().unwrap().len())
            .sum();
        assert!(
            records.len() < 16 && bytes <= 8 * 1024 * 1024,
            "unbounded copy records: {} files, {bytes} bytes",
            records.len()
        );
        assert!(
            !snapshot.exists(),
            "verified restoration must retire its backup"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[tokio::test]
async fn claude_copy_fallback_verifies_collapsed_multiline_and_rejects_edited_words() {
    const DRAFT: &str = "review the changes\n\nbefore publishing";
    for (readback, restored) in [
        ("review the changes before merging", false),
        ("review the changes  before publishing", true),
    ] {
        let directory = crate::utils::test_temp::unique_temp_path("claude-copy-collapsed-readback");
        let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
        let clock = runtime.test_submission_clock().unwrap();
        clock.pause();
        let (completion, events) = captured_copy_with_draft(&runtime, directory.clone(), DRAFT);
        accept_copy_message(&runtime, &mut writes).await;
        paint(&runtime, "", false, "");
        assert_eq!(
            key(&mut writes).await.as_ref(),
            b"\x1b[200~review the changes\n\nbefore publishing\x1b[201~"
        );
        paint(&runtime, readback, false, "");
        clock.advance(Duration::from_secs(2));
        completion
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        if restored {
            copy_event(&events, "bus.draft.copy_recovery_retired");
            assert!(!directory.exists() || std::fs::read_dir(&directory).unwrap().next().is_none());
        } else {
            let event = copy_event(&events, "copy_restore_failed");
            let snapshot = copy_record(&directory, DRAFT);
            assert!(event.contains(&*snapshot.to_string_lossy()), "{event}");
            assert_eq!(std::fs::read_to_string(snapshot).unwrap(), DRAFT);
        }
        assert!(writes.try_recv().is_err());
        if directory.exists() {
            std::fs::remove_dir_all(directory).unwrap();
        }
    }
}

fn pause_copy_write(
    directory: &std::path::Path,
) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
    let (started, waiting) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    crate::terminal::runtime::copy_recovery_test_hooks::install(directory.to_owned(), move || {
        started.send(()).map_err(std::io::Error::other)?;
        released
            .recv_timeout(Duration::from_secs(8))
            .map_err(std::io::Error::other)
    });
    (waiting, release)
}

async fn restore_copy_message(
    runtime: &TerminalRuntime,
    writes: &mut tokio::sync::mpsc::Receiver<Bytes>,
) {
    accept_copy_message(runtime, writes).await;
    paint(runtime, "", false, "");
    assert_eq!(
        key(writes).await.as_ref(),
        b"\x1b[200~developer draft\x1b[201~"
    );
    paint(runtime, COPIED_DRAFT, false, "");
}

#[tokio::test]
async fn claude_copy_fallback_saturation_keeps_terminal_retirement() {
    use crate::terminal::runtime::copy_recovery_test_hooks as hooks;
    let scratch = crate::utils::test_temp::unique_temp_path("copy-saturated");
    let blocker = scratch.join("blocker");
    let pane = scratch.join("pane");
    crate::platform::fs::private_dir(&pane).unwrap();
    let previous = copy_record(&pane, COPIED_DRAFT);
    write_private_copy(&previous, COPIED_DRAFT);
    let (waiting, release) = pause_copy_write(&blocker);
    hooks::retain(&blocker, "independent slow snapshot");
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();

    let (old, mut old_writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    old.test_submission_clock().unwrap().pause();
    let (old_result, old_events) = captured_copy(&old, pane.clone());
    accept_copy_message(&old, &mut old_writes).await;
    old.invalidate_submission_ownership();
    paint(&old, "", false, "");
    old_result
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();

    for slot in 0..15 {
        hooks::retain(
            &scratch.join(format!("queued-{slot}")),
            &format!("queued snapshot {slot}"),
        );
    }
    let (retry, mut retry_writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    retry.test_submission_clock().unwrap().pause();
    let (retry_result, retry_events) = captured_copy(&retry, pane.clone());
    restore_copy_message(&retry, &mut retry_writes).await;
    retry_result
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert!(retry_writes.try_recv().is_err());
    release.send(()).unwrap();

    // Both tracing dispatches close only after the old persistence job and
    // this retry's terminal retirement have been handled.
    let mut notices = Vec::new();
    for events in [old_events, retry_events] {
        while let Ok(line) = events.recv_timeout(Duration::from_secs(5)) {
            notices.push(line);
        }
    }
    assert!(
        !notices
            .iter()
            .any(|line| line.contains("bus.draft.recovery_required")),
        "{notices:?}"
    );
    assert!(
        notices
            .iter()
            .any(|line| line.contains("snapshot_worker_busy")),
        "{notices:?}"
    );
    assert!(
        notices
            .iter()
            .any(|line| line.contains("bus.draft.copy_recovery_retired")),
        "{notices:?}"
    );
    assert!(!previous.exists());
    std::fs::remove_dir_all(scratch).unwrap();
}

fn full_copy_directory(directory: &std::path::Path) -> std::path::PathBuf {
    crate::platform::fs::private_dir(directory).unwrap();
    let oldest = copy_record(directory, COPIED_DRAFT);
    write_private_copy(&oldest, COPIED_DRAFT);
    // Target A's modification time first so it would be the eviction victim.
    std::fs::File::options()
        .write(true)
        .open(&oldest)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH)
        .unwrap();
    for i in 0..15 {
        let text = format!("retained snapshot {i}");
        write_private_copy(&copy_record(directory, &text), &text);
    }
    oldest
}

#[tokio::test]
async fn claude_copy_fallback_failed_replacement_never_evicts_oldest() {
    let directory = crate::utils::test_temp::unique_temp_path("copy-failed-replacement");
    let oldest = full_copy_directory(&directory);
    crate::terminal::runtime::copy_recovery_test_hooks::install(directory.clone(), || {
        Err(std::io::Error::other(
            "injected replacement publication failure",
        ))
    });
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    runtime.test_submission_clock().unwrap().pause();
    let (result, events) =
        captured_copy_with_draft(&runtime, directory.clone(), "distinct snapshot B");
    accept_copy_message(&runtime, &mut writes).await;
    runtime.invalidate_submission_ownership();
    paint(&runtime, "", false, "");
    result
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    copy_event(&events, "snapshot_storage_failed");
    assert_eq!(std::fs::read_to_string(oldest).unwrap(), COPIED_DRAFT);
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 16);
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn claude_copy_fallback_replacement_protects_in_flight_oldest() {
    let scratch = crate::utils::test_temp::unique_temp_path("copy-active-eviction");
    let directory = scratch.join("pane");
    let blocker = scratch.join("blocker");
    let oldest = full_copy_directory(&directory);
    let (replacing, mut b_writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    replacing.test_submission_clock().unwrap().pause();
    // Hold an unrelated worker write so A is in flight before B begins
    // either replacement publication or any eviction.
    let (paused, release) = pause_copy_write(&blocker);
    crate::terminal::runtime::copy_recovery_test_hooks::retain(
        &blocker,
        "independent slow snapshot",
    );
    paused.recv_timeout(Duration::from_secs(5)).unwrap();
    let (b_result, b_events) =
        captured_copy_with_draft(&replacing, directory.clone(), "distinct snapshot B");
    accept_copy_message(&replacing, &mut b_writes).await;
    replacing.invalidate_submission_ownership();
    paint(&replacing, "", false, "");
    b_result
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    let (retry, mut a_writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    retry.test_submission_clock().unwrap().pause();
    let (a_result, a_events) = captured_copy(&retry, directory.clone());
    accept_copy_message(&retry, &mut a_writes).await;
    release.send(()).unwrap();
    copy_event(&b_events, "bus.draft.recovery_required");
    let protected = std::fs::read_to_string(&oldest);

    paint(&retry, "", false, "");
    assert_eq!(
        key(&mut a_writes).await.as_ref(),
        b"\x1b[200~developer draft\x1b[201~"
    );
    paint(&retry, COPIED_DRAFT, false, "");
    a_result
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    copy_event(&a_events, "bus.draft.copy_recovery_retired");
    assert_eq!(protected.unwrap(), COPIED_DRAFT);
    assert!(!oldest.exists());
    std::fs::remove_dir_all(scratch).unwrap();
}

struct BlockedCopyLog {
    writer: CopyLogWriter,
    gate: SharedCopyLogGate,
}

type SharedCopyLogGate = std::sync::Arc<
    std::sync::Mutex<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>>,
>;

impl std::io::Write for BlockedCopyLog {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.writer.write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}

impl Drop for BlockedCopyLog {
    fn drop(&mut self) {
        if String::from_utf8_lossy(&self.writer.line).contains("bus.draft.recovery_required") {
            let gate = self.gate.lock().unwrap().take();
            if let Some((started, release)) = gate {
                started.send(()).unwrap();
                release.recv_timeout(Duration::from_secs(8)).unwrap();
            }
        }
    }
}

#[tokio::test]
async fn claude_copy_fallback_blocked_recovery_notice_does_not_delay_other_pane_delivery() {
    let scratch = crate::utils::test_temp::unique_temp_path("copy-blocked-sink-pty");
    let (messages, events) = std::sync::mpsc::channel();
    let (started, waiting) = std::sync::mpsc::channel();
    let (release, resume) = std::sync::mpsc::channel();
    let gate = std::sync::Arc::new(std::sync::Mutex::new(Some((started, resume))));
    let dispatch = tracing::Dispatch::new(
        tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || BlockedCopyLog {
                writer: CopyLogWriter {
                    messages: messages.clone(),
                    line: Vec::new(),
                },
                gate: gate.clone(),
            })
            .finish(),
    );
    let (first, mut first_keys) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    first.test_submission_clock().unwrap().pause();
    let first_result = tracing::dispatcher::with_default(&dispatch, || {
        queue_claude_copy(&first, scratch.join("first"), COPIED_DRAFT)
    });
    accept_copy_message(&first, &mut first_keys).await;
    first.invalidate_submission_ownership();
    paint(&first, "", false, "");
    first_result
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();

    let (second, mut second_keys) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    second.test_submission_clock().unwrap().pause();
    let (second_result, second_events) = captured_copy(&second, scratch.join("second"));
    let delivery = tokio::time::timeout(Duration::from_secs(2), async {
        restore_copy_message(&second, &mut second_keys).await;
        tokio::task::spawn_blocking(move || second_result.recv_timeout(Duration::from_secs(2)))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    })
    .await;
    release.send(()).unwrap();
    copy_event(&events, "bus.draft.recovery_required");
    assert!(
        delivery.is_ok(),
        "a blocked recovery notice delayed the other pane"
    );
    copy_event(&second_events, "bus.draft.copy_recovery_retired");
    assert!(second_keys.try_recv().is_err());
    std::fs::remove_dir_all(scratch).unwrap();
}
