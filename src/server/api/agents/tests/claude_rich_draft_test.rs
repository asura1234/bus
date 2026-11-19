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

async fn clear_restored(
    runtime: &TerminalRuntime,
    writes: &mut tokio::sync::mpsc::Receiver<Bytes>,
    initial: &str,
) {
    let mut body = initial.to_owned();
    let mut cursor = body.len();
    for _ in 0..30 {
        if body.is_empty() {
            return;
        }
        let start = body[..cursor].rfind('\n').map_or(0, |at| at + 1);
        let end = body[cursor..]
            .find('\n')
            .map_or(body.len(), |at| cursor + at);
        match key(writes).await.as_ref() {
            b"\x01" => cursor = start,
            b"\x0b" => {
                let end = if cursor == end && end < body.len() {
                    end + 1
                } else {
                    end
                };
                body.replace_range(cursor..end, "");
            }
            b"\x15" => {
                let start = if cursor == start {
                    start.saturating_sub(1)
                } else {
                    start
                };
                body.replace_range(start..cursor, "");
                cursor = start;
            }
            other => panic!("retirement sent paste, Enter, or stash: {other:?}"),
        }
        paint(runtime, &body, false, "retiring owned prompt");
    }
    panic!("retirement did not finish");
}

async fn confirmed_plain(
    runtime: &TerminalRuntime,
    writes: &mut tokio::sync::mpsc::Receiver<Bytes>,
    directory: &std::path::Path,
    payload: &str,
) {
    runtime
        .submission_handle(true)
        .bind_claude_session("session");
    submit_plain(runtime, writes, directory, payload, payload, false).await;
}

async fn submit_plain(
    runtime: &TerminalRuntime,
    writes: &mut tokio::sync::mpsc::Receiver<Bytes>,
    directory: &std::path::Path,
    payload: &str,
    rendered: &str,
    steer: bool,
) {
    paint(runtime, "", false, "idle");
    let completion = runtime
        .queue_observed_input_with_inspector(
            Bytes::copy_from_slice(payload.as_bytes()),
            Bytes::from_static(b"\r"),
            vec![
                Bytes::from_static(b"\x01"),
                Bytes::from_static(b"\x0b"),
                Bytes::from_static(b"\x15"),
            ],
            Duration::ZERO,
            None,
            Box::new(
                PreservingComposer::new(
                    AgentKind::Claude,
                    payload.into(),
                    directory.to_owned(),
                    vec![
                        Bytes::from_static(b"\x13"),
                        Bytes::from_static(b"\x07"),
                        Bytes::from_static(b"\x7f"),
                    ],
                )
                .with_steering(steer),
            ),
        )
        .unwrap();
    assert_eq!(key(writes).await.as_ref(), payload.as_bytes());
    paint(runtime, rendered, false, "owned");
    assert_eq!(key(writes).await.as_ref(), b"\r");
    paint(runtime, "", false, "working");
    completion
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn claude_interrupt_retirement_preserves_foreign_and_whitespace_edited_drafts() {
    for body in ["other developer draft", "from  bus", "from bus edited"] {
        let directory = crate::utils::test_temp::unique_temp_path("claude-foreign-retirement");
        let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
        confirmed_plain(&runtime, &mut writes, &directory, "from bus").await;
        let sender = runtime.submission_handle(true);
        let stopped = tokio::task::spawn_blocking(move || {
            sender.retire_claude_interrupt(
                "session",
                &["from bus".into()],
                Bytes::from_static(b"\x1b"),
            )
        });
        assert_eq!(key(&mut writes).await.as_ref(), b"\x1b");
        paint(&runtime, body, false, "restored changed input");
        assert!(stopped.await.unwrap().is_err());
        assert!(
            writes.try_recv().is_err(),
            "foreign input received editing keys"
        );
    }
}

#[tokio::test]
async fn claude_interrupt_retirement_rejects_session_or_payload_replacement_and_developer_input() {
    for case in ["session", "payload", "developer"] {
        let directory = crate::utils::test_temp::unique_temp_path("claude-retirement-identity");
        let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
        confirmed_plain(&runtime, &mut writes, &directory, "from bus").await;
        if case == "developer" {
            runtime
                .submission_handle(true)
                .try_send_developer_bytes(Bytes::from_static(b"x"))
                .unwrap();
            assert_eq!(key(&mut writes).await.as_ref(), b"x");
        }
        let result = runtime.submission_handle(true).retire_claude_interrupt(
            if case == "session" {
                "other-session"
            } else {
                "session"
            },
            &[if case == "payload" {
                "new request".into()
            } else {
                "from bus".into()
            }],
            Bytes::from_static(b"\x1b"),
        );
        assert!(result.is_err());
        assert!(
            writes.try_recv().is_err(),
            "identity mismatch must not press stop or clear"
        );
    }
}

#[tokio::test]
async fn claude_interrupt_empty_repaint_is_unconfirmed_and_retry_never_represses_stop() {
    let directory = crate::utils::test_temp::unique_temp_path("claude-empty-retirement");
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    confirmed_plain(&runtime, &mut writes, &directory, "from bus").await;
    let sender = runtime.submission_handle(true);
    let stopped = tokio::task::spawn_blocking(move || {
        sender.retire_claude_interrupt("session", &["from bus".into()], Bytes::from_static(b"\x1b"))
    });
    assert_eq!(key(&mut writes).await.as_ref(), b"\x1b");
    paint(&runtime, "", false, "idle; no restoration yet");
    assert!(
        stopped.await.unwrap().is_err(),
        "empty is not retirement proof"
    );
    assert!(writes.try_recv().is_err());
    let sender = runtime.submission_handle(true);
    let retried = tokio::task::spawn_blocking(move || {
        sender.retire_claude_interrupt("session", &["from bus".into()], Bytes::from_static(b"\x1b"))
    });
    paint(&runtime, "from bus", false, "late restoration");
    clear_restored(&runtime, &mut writes, "from bus").await;
    retried.await.unwrap().unwrap();
    assert!(writes.try_recv().is_err(), "one Esc; no paste or Enter");
}

#[tokio::test]
async fn claude_interrupt_lost_stop_receipt_preserves_ownership_without_replaying_esc() {
    let directory = crate::utils::test_temp::unique_temp_path("claude-lost-stop-receipt");
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    confirmed_plain(&runtime, &mut writes, &directory, "from bus").await;
    let release = runtime
        .test_submission_clock()
        .unwrap()
        .fail_next_write_completion(std::io::ErrorKind::BrokenPipe);
    let sender = runtime.submission_handle(true);
    let stopped = tokio::task::spawn_blocking(move || {
        sender.retire_claude_interrupt("session", &["from bus".into()], Bytes::from_static(b"\x1b"))
    });
    assert_eq!(key(&mut writes).await.as_ref(), b"\x1b");
    paint(
        &runtime,
        "from bus",
        false,
        "stop restored input but acknowledgement lost",
    );
    release.send(()).unwrap();
    assert!(stopped.await.unwrap().is_err());
    assert!(writes.try_recv().is_err());
    let sender = runtime.submission_handle(true);
    let retry = tokio::task::spawn_blocking(move || {
        sender.retire_claude_interrupt("session", &["from bus".into()], Bytes::from_static(b"\x1b"))
    });
    clear_restored(&runtime, &mut writes, "from bus").await;
    retry.await.unwrap().unwrap();
    assert!(
        writes.try_recv().is_err(),
        "lost acknowledgement must not replay stop, paste or Enter"
    );
}

#[tokio::test]
async fn claude_interrupt_buffers_developer_input_and_revokes_retirement() {
    let directory = crate::utils::test_temp::unique_temp_path("claude-edit-during-retirement");
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    confirmed_plain(&runtime, &mut writes, &directory, "from bus").await;
    let release = runtime
        .test_submission_clock()
        .unwrap()
        .hold_next_write_completion();
    let sender = runtime.submission_handle(true);
    let stopped = tokio::task::spawn_blocking(move || {
        sender.retire_claude_interrupt("session", &["from bus".into()], Bytes::from_static(b"\x1b"))
    });
    assert_eq!(key(&mut writes).await.as_ref(), b"\x1b");
    runtime
        .submission_handle(true)
        .try_send_developer_bytes(Bytes::from_static(b"x"))
        .unwrap();
    paint(&runtime, "from bus", false, "restored input");
    release.send(()).unwrap();
    assert!(stopped.await.unwrap().is_err());
    assert_eq!(key(&mut writes).await.as_ref(), b"x");
    assert!(
        writes.try_recv().is_err(),
        "developer input must revoke all cleanup keys"
    );
}

#[tokio::test]
async fn claude_interrupt_cannot_retire_an_inflight_submission_before_its_receipt() {
    let directory = crate::utils::test_temp::unique_temp_path("claude-submit-inflight-retirement");
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    runtime
        .submission_handle(true)
        .bind_claude_session("session");
    paint(&runtime, "", false, "idle");
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
                directory,
                vec![
                    Bytes::from_static(b"\x13"),
                    Bytes::from_static(b"\x07"),
                    Bytes::from_static(b"\x7f"),
                ],
            )),
        )
        .unwrap();
    assert_eq!(key(&mut writes).await.as_ref(), b"from bus");
    assert!(runtime
        .submission_handle(true)
        .retire_claude_interrupt("session", &["from bus".into()], Bytes::from_static(b"\x1b"))
        .is_err());
    assert!(
        writes.try_recv().is_err(),
        "no stop while another input lease owns the pane"
    );
    paint(&runtime, "from bus", false, "owned");
    assert_eq!(key(&mut writes).await.as_ref(), b"\r");
    paint(&runtime, "", false, "working");
    completion
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert!(writes.try_recv().is_err());
}

/// Incident 50780/50781: Esc restores an accepted Bus prompt. A later send
/// must retire that owned prompt instead of stashing/restoring it as a draft.
#[tokio::test]
async fn claude_ds_interrupted_bus_draft_is_retired_before_a_followup_can_restore_it() {
    assert!(crate::agents::providers::ProviderKind::ClaudeDs.uses_claude_harness());
    let directory = crate::utils::test_temp::unique_temp_path("claude-ds-interrupted-draft");
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    runtime
        .submission_handle(true)
        .bind_claude_session("session");
    let old = "[Bus message 50780 from you]\nwhat's going on with the playhead center?";
    let submit = |text: &str| {
        runtime
            .queue_observed_input_with_inspector(
                Bytes::copy_from_slice(text.as_bytes()),
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
                    text.into(),
                    directory.clone(),
                    vec![
                        Bytes::from_static(b"\x13"),
                        Bytes::from_static(b"\x07"),
                        Bytes::from_static(b"\x7f"),
                    ],
                )),
            )
            .unwrap()
    };
    paint(&runtime, "", false, "idle");
    let first = submit(old);
    assert_eq!(key(&mut writes).await.as_ref(), old.as_bytes());
    paint(&runtime, old, false, "paste rendered");
    assert_eq!(key(&mut writes).await.as_ref(), b"\r");
    paint(&runtime, "", false, "working");
    first.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();

    // Bus stop is a fenced transaction; a developer Esc remains editing.
    let sender = runtime.submission_handle(true);
    let payload = old.to_owned();
    let stopping = tokio::task::spawn_blocking(move || {
        sender.retire_claude_interrupt("session", &[payload], Bytes::from_static(b"\x1b"))
    });
    assert_eq!(key(&mut writes).await.as_ref(), b"\x1b");
    paint(
        &runtime,
        old,
        false,
        "interrupted; original prompt restored",
    );
    clear_restored(&runtime, &mut writes, old).await;
    stopping.await.unwrap().unwrap();
    let followup = submit("[Bus message 50782 from you]\nis that one merged?");
    assert_eq!(
        key(&mut writes).await.as_ref(),
        b"[Bus message 50782 from you]\nis that one merged?"
    );
    paint(
        &runtime,
        "[Bus message 50782 from you]\nis that one merged?",
        false,
        "followup",
    );
    assert_eq!(key(&mut writes).await.as_ref(), b"\r");
    paint(&runtime, "", false, "working");
    followup
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert!(
        writes.try_recv().is_err(),
        "no old paste, extra Enter, or stash toggle"
    );
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

#[tokio::test]
async fn lost_retirement_response_is_idempotent() {
    let directory = crate::utils::test_temp::unique_temp_path("claude-lost-retirement-response");
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    confirmed_plain(&runtime, &mut writes, &directory, "from bus").await;
    let sender = runtime.submission_handle(true);
    let stopped = tokio::task::spawn_blocking(move || {
        sender.retire_claude_interrupt("session", &["from bus".into()], Bytes::from_static(b"\x1b"))
    });
    assert_eq!(key(&mut writes).await.as_ref(), b"\x1b");
    paint(&runtime, "from bus", false, "restored");
    clear_restored(&runtime, &mut writes, "from bus").await;
    stopped.await.unwrap().unwrap();
    // The coordinator did not receive the result. A retry must prove completion.
    runtime
        .submission_handle(true)
        .retire_claude_interrupt("session", &["from bus".into()], Bytes::from_static(b"\x1b"))
        .unwrap();
    assert!(writes.try_recv().is_err());
}

#[tokio::test]
async fn partial_retirement_resumes_only_an_owned_remainder() {
    let directory = crate::utils::test_temp::unique_temp_path("claude-lost-partial-clear");
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    let payload = "first\nsecond";
    confirmed_plain(&runtime, &mut writes, &directory, payload).await;
    let sender = runtime.submission_handle(true);
    let clock = runtime.test_submission_clock().unwrap();
    let esc = clock.hold_next_write_completion();
    let stopped = tokio::task::spawn_blocking(move || {
        sender.retire_claude_interrupt("session", &[payload.into()], Bytes::from_static(b"\x1b"))
    });
    assert_eq!(key(&mut writes).await.as_ref(), b"\x1b");
    paint(&runtime, payload, false, "restored");
    let start = clock.hold_next_write_completion();
    esc.send(()).unwrap();
    assert_eq!(key(&mut writes).await.as_ref(), b"\x01");
    let lost = clock.fail_next_write_completion(std::io::ErrorKind::BrokenPipe);
    start.send(()).unwrap();
    assert_eq!(key(&mut writes).await.as_ref(), b"\x0b");
    paint(&runtime, "first\n", false, "partly cleared");
    lost.send(()).unwrap();
    assert!(stopped.await.unwrap().is_err());
    let sender = runtime.submission_handle(true);
    let retry = tokio::task::spawn_blocking(move || {
        sender.retire_claude_interrupt("session", &[payload.into()], Bytes::from_static(b"\x1b"))
    });
    // The fake provider's cursor is still on the empty second line.
    clear_restored(&runtime, &mut writes, "first\n").await;
    retry.await.unwrap().unwrap();
    assert!(writes.try_recv().is_err());
}

#[tokio::test]
async fn completed_retirement_receipt_survives_a_new_runtime() {
    let directory = crate::utils::test_temp::unique_temp_path("claude-completion-restart");
    let receipt = directory.join("completion");
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    confirmed_plain(&runtime, &mut writes, &directory, "from bus").await;
    let sender = runtime.submission_handle(true);
    let path = receipt.clone();
    let stopped = tokio::task::spawn_blocking(move || {
        sender.retire_claude_interrupt_with_receipt(
            "session",
            &["from bus".into()],
            Bytes::from_static(b"\x1b"),
            Some(&path),
        )
    });
    assert_eq!(key(&mut writes).await.as_ref(), b"\x1b");
    paint(&runtime, "from bus", false, "restored");
    clear_restored(&runtime, &mut writes, "from bus").await;
    stopped.await.unwrap().unwrap();
    drop(runtime);
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    let sender = runtime.submission_handle(true);
    sender.bind_claude_session("session");
    assert!(sender
        .retire_claude_interrupt_with_receipt(
            "session",
            &["other request".into()],
            Bytes::from_static(b"\x1b"),
            Some(&directory.join("different-retirement"))
        )
        .is_err());
    sender
        .retire_claude_interrupt_with_receipt(
            "session",
            &["from bus".into()],
            Bytes::from_static(b"\x1b"),
            Some(&receipt),
        )
        .unwrap();
    assert!(writes.try_recv().is_err());
}

#[tokio::test]
async fn claude_retirement_rejects_empty_frame_after_cursor_motion_alone() {
    let directory = crate::utils::test_temp::unique_temp_path("claude-transient-clear-frame");
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    confirmed_plain(&runtime, &mut writes, &directory, "from bus").await;
    let sender = runtime.submission_handle(true);
    let clock = runtime.test_submission_clock().unwrap();
    let esc = clock.hold_next_write_completion();
    let stopped = tokio::task::spawn_blocking(move || {
        sender.retire_claude_interrupt("session", &["from bus".into()], Bytes::from_static(b"\x1b"))
    });
    assert_eq!(key(&mut writes).await.as_ref(), b"\x1b");
    paint(&runtime, "from bus", false, "restored");
    let cursor = clock.hold_next_write_completion();
    esc.send(()).unwrap();
    assert_eq!(key(&mut writes).await.as_ref(), b"\x01");
    paint(&runtime, "", false, "transient blank frame");
    cursor.send(()).unwrap();
    assert!(stopped.await.unwrap().is_err());
    assert!(writes.try_recv().is_err());
}

#[tokio::test]
async fn lost_final_clear_receipt_uses_fresh_empty_without_another_key() {
    let directory = crate::utils::test_temp::unique_temp_path("claude-lost-final-clear");
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    confirmed_plain(&runtime, &mut writes, &directory, "from bus").await;
    let sender = runtime.submission_handle(true);
    let clock = runtime.test_submission_clock().unwrap();
    let esc = clock.hold_next_write_completion();
    let stopped = tokio::task::spawn_blocking(move || {
        sender.retire_claude_interrupt("session", &["from bus".into()], Bytes::from_static(b"\x1b"))
    });
    assert_eq!(key(&mut writes).await.as_ref(), b"\x1b");
    paint(&runtime, "from bus", false, "restored");
    let cursor = clock.hold_next_write_completion();
    esc.send(()).unwrap();
    assert_eq!(key(&mut writes).await.as_ref(), b"\x01");
    let lost = clock.fail_next_write_completion(std::io::ErrorKind::BrokenPipe);
    cursor.send(()).unwrap();
    assert_eq!(key(&mut writes).await.as_ref(), b"\x0b");
    paint(&runtime, "", false, "cleared");
    lost.send(()).unwrap();
    assert!(stopped.await.unwrap().is_err());
    runtime
        .submission_handle(true)
        .retire_claude_interrupt("session", &["from bus".into()], Bytes::from_static(b"\x1b"))
        .unwrap();
    assert!(writes.try_recv().is_err());
}

#[tokio::test]
async fn claude_ds_busy_and_unbound_delivery_retain_the_admitted_input_witness() {
    // ClaudeDs uses the Claude terminal harness. Exercise its unbound observed
    // submission and busy-steering observer without starting a provider.
    for steer in [false, true] {
        let directory = crate::utils::test_temp::unique_temp_path("claude-late-receipt");
        let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
        let payload = "from bus";
        submit_plain(&runtime, &mut writes, &directory, payload, payload, steer).await;
        let sender = runtime.submission_handle(true);
        sender.bind_claude_session("session");
        let mut retirement = tokio::task::spawn_blocking(move || {
            sender.retire_claude_interrupt(
                "session",
                &[payload.into()],
                Bytes::from_static(b"\x1b"),
            )
        });
        tokio::select! {
            result = &mut retirement => panic!("retirement refused before Esc: {result:?}"),
            first_key = key(&mut writes) => assert_eq!(first_key.as_ref(), b"\x1b"),
        }
        paint(&runtime, payload, false, "restored");
        clear_restored(&runtime, &mut writes, payload).await;
        retirement.await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn claude_ds_interrupt_retires_exact_soft_wrapped_restoration_after_ctrl_k() {
    let directory = crate::utils::test_temp::unique_temp_path("claude-wrapped-retirement");
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    let prefix = "bus master:\n[Bus message 51165 from bus master (room 0)]";
    let payload = format!("{prefix}\nRun the shell command sleep 20, then reply with exactly FIRST_DONE. Do not do anything else.");
    let wrapped = format!("{prefix}\nRun the shell command sleep 20, then reply with exactly FIRST_DONE. Do not do anything\nelse.");
    runtime
        .submission_handle(true)
        .bind_claude_session("session");
    submit_plain(&runtime, &mut writes, &directory, &payload, &wrapped, false).await;
    let sender = runtime.submission_handle(true);
    let mut retirement = tokio::task::spawn_blocking(move || {
        sender.retire_claude_interrupt("session", &[payload], Bytes::from_static(b"\x1b"))
    });
    assert_eq!(key(&mut writes).await.as_ref(), b"\x1b");
    // The submission and restoration receipts must describe the same frame.
    paint(&runtime, &wrapped, false, "restored");
    assert_eq!(key(&mut writes).await.as_ref(), b"\x01");
    // Ctrl+A placed the cursor on the first content cell of the wrapped line.
    runtime.test_process_pty_bytes(b"\x1b[6;1H");
    assert_eq!(key(&mut writes).await.as_ref(), b"\x0b");
    paint(&runtime, &format!("{prefix}\nelse."), false, "restored");
    tokio::select! {
        result = &mut retirement => panic!("exact post-Ctrl+K frame was refused: {result:?}"),
        next = key(&mut writes) => assert_eq!(next.as_ref(), b"\x15"),
    }
    // A new coherent frame with unrelated text must still prevent any next key.
    paint(&runtime, "developer's draft", false, "edited");
    assert!(retirement.await.unwrap().is_err());
    assert!(writes.try_recv().is_err());
}

#[tokio::test]
async fn a_busy_join_preserves_the_leads_exact_restoration_witness() {
    let directory = crate::utils::test_temp::unique_temp_path("claude-joined-receipt");
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    confirmed_plain(&runtime, &mut writes, &directory, "lead payload").await;
    submit_plain(
        &runtime,
        &mut writes,
        &directory,
        "joined payload",
        "joined payload",
        true,
    )
    .await;
    let sender = runtime.submission_handle(true);
    let mut retirement = tokio::task::spawn_blocking(move || {
        sender.retire_claude_interrupt(
            "session",
            &["lead payload".into(), "joined payload".into()],
            Bytes::from_static(b"\x1b"),
        )
    });
    assert_eq!(key(&mut writes).await.as_ref(), b"\x1b");
    paint(&runtime, "lead payload", false, "restored");
    tokio::select! {
        result = &mut retirement => panic!("lost the lead's owned draft: {result:?}"),
        _ = clear_restored(&runtime, &mut writes, "lead payload") => {}
    }
    retirement.await.unwrap().unwrap();
}
