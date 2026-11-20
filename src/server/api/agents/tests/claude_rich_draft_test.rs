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
