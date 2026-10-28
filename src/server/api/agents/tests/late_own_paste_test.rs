use super::*;
use crate::terminal::pty::actor::InputCompletion;
use crate::terminal::TerminalRuntime;
use bytes::Bytes;
use std::time::{Duration, Instant};

const WAIT: Duration = crate::terminal::runtime::NATIVE_TEST_TIMEOUT;
const NEXT: &str = "later room message";

fn frame(kind: AgentKind, body: &str) -> String {
    let body = body.replace('\n', "\r\n  ");
    match kind {
        AgentKind::Claude => format!("\x1b[2J\x1b[H────────────────────────\r\n❯ {body}\r\n────────────────────────\r\n⏵⏵ auto mode on (shift+tab to cycle) · ← 1 agent"),
        AgentKind::Codex => format!("\x1b[2J\x1b[H› {body}\r\n GPT-6 · /repo\r\n ? for shortcuts"),
        AgentKind::Cursor => format!("\x1b[2J\x1b[H > {body}\r\n / commands · @ files"),
        _ => unreachable!(),
    }
}

fn show(runtime: &TerminalRuntime, kind: AgentKind, body: &str) {
    runtime.test_process_pty_bytes(frame(kind, body).as_bytes());
}

fn send(
    runtime: &TerminalRuntime,
    kind: AgentKind,
    text: &str,
    deadline: Option<Instant>,
) -> InputCompletion {
    super::super::super::queue_prompt_input(
        runtime,
        kind,
        text.into(),
        true,
        Duration::ZERO,
        deadline,
        None,
    )
    .unwrap()
}

async fn input_or_result(
    writes: &mut tokio::sync::mpsc::Receiver<Bytes>,
    result: &mut tokio::task::JoinHandle<std::io::Result<()>>,
) -> Bytes {
    tokio::select! {
        write = writes.recv() => write.expect("input channel closed"),
        result = result => panic!("later message refused instead of recovering the late own paste: {result:?}"),
    }
}

async fn uncertain(
    runtime: &TerminalRuntime,
    writes: &mut tokio::sync::mpsc::Receiver<Bytes>,
    kind: AgentKind,
    text: &str,
    partial: Option<&str>,
    under_load: bool,
) {
    let clock = runtime.test_submission_clock().unwrap();
    let deadline = under_load.then(|| clock.now() + Duration::from_millis(1400));
    let result = send(runtime, kind, text, deadline);
    let paste = tokio::time::timeout(WAIT, writes.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(paste.starts_with(b"\x1b[200~"));
    clock.wait_for_draft_wait();
    if let Some(partial) = partial {
        show(runtime, kind, partial);
    }
    clock.advance(if under_load {
        Duration::from_millis(500)
    } else {
        Duration::from_secs(3)
    });
    let error = result.recv_timeout(WAIT).unwrap().unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    assert!(
        writes.try_recv().is_err(),
        "no Enter or cleanup for an unseen/partial paste"
    );
}

async fn recover_and_deliver(
    runtime: &TerminalRuntime,
    writes: &mut tokio::sync::mpsc::Receiver<Bytes>,
    kind: AgentKind,
    text: &str,
    collapsed: bool,
) {
    let completion = send(runtime, kind, NEXT, None);
    let result = tokio::task::spawn_blocking(move || completion.recv_timeout(WAIT).unwrap());
    finish_recovery(runtime, writes, kind, text, collapsed, result).await;
}

async fn finish_recovery(
    runtime: &TerminalRuntime,
    writes: &mut tokio::sync::mpsc::Receiver<Bytes>,
    kind: AgentKind,
    text: &str,
    collapsed: bool,
    mut result: tokio::task::JoinHandle<std::io::Result<()>>,
) {
    let mut remaining = text.to_owned();
    let mut cursor = remaining.len();
    loop {
        let bytes = input_or_result(writes, &mut result).await;
        if bytes.starts_with(b"\x1b[200~") {
            assert!(
                remaining.is_empty(),
                "later paste overwrote an uncleared old draft"
            );
            assert_eq!(bytes, Bytes::from(format!("\x1b[200~{NEXT}\x1b[201~")));
            break;
        }
        assert_ne!(bytes.as_ref(), b"\r", "never submit the abandoned message");
        let start = remaining[..cursor].rfind('\n').map_or(0, |at| at + 1);
        let end = remaining[cursor..]
            .find('\n')
            .map_or(remaining.len(), |at| cursor + at);
        let before = remaining.clone();
        match bytes.as_ref() {
            b"\x01" => cursor = if collapsed { 0 } else { start },
            b"\x0b" if collapsed => {
                remaining.clear();
                cursor = 0;
            }
            b"\x0b" => {
                let end = if cursor == end && end < remaining.len() {
                    end + 1
                } else {
                    end
                };
                remaining.replace_range(cursor..end, "");
            }
            b"\x15" => {
                let start = if cursor == start {
                    start.saturating_sub(1)
                } else {
                    start
                };
                remaining.replace_range(start..cursor, "");
                cursor = start;
            }
            _ => panic!("unexpected recovery input {bytes:?}"),
        }
        if remaining != before {
            show(runtime, kind, &remaining);
        } else {
            // Cursor-only keys do not repaint the draft. Advance the fake
            // provider clock so the writer's inter-key delay can complete.
            runtime
                .test_submission_clock()
                .unwrap()
                .advance(Duration::from_millis(25));
        }
    }
    show(runtime, kind, NEXT);
    assert_eq!(input_or_result(writes, &mut result).await.as_ref(), b"\r");
    show(runtime, kind, "");
    assert!(result.await.unwrap().is_ok());
    assert!(writes.try_recv().is_err(), "later message submitted once");
}

async fn late_case(
    kind: AgentKind,
    text: &str,
    body: &str,
    partial: Option<&str>,
    collapsed: bool,
    under_load: bool,
) {
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(123, 48, 32);
    runtime.test_process_pty_bytes(b"\x1b[?2004h");
    show(&runtime, kind, "");
    runtime.test_submission_clock().unwrap().pause();
    uncertain(&runtime, &mut writes, kind, text, partial, under_load).await;
    show(&runtime, kind, body);
    recover_and_deliver(&runtime, &mut writes, kind, text, collapsed).await;
}

#[tokio::test]
async fn incident_claude_paste_rendered_after_timeout_is_recovered_before_the_next_room_message() {
    let text = include_str!("fixtures/stale_bus_cjk_payload.txt");
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(123, 48, 32);
    runtime.test_process_pty_bytes(b"\x1b[?2004h");
    show(&runtime, AgentKind::Claude, "");
    runtime.test_submission_clock().unwrap().pause();
    uncertain(&runtime, &mut writes, AgentKind::Claude, text, None, false).await;
    // Replay the actual provider output only after the old attempt completed.
    runtime.test_process_pty_bytes(b"\x1bc\x1b[?2004h");
    runtime.test_process_pty_bytes(include_bytes!(
        "fixtures/claude_2_1_310_cjk_123cols_color.pty"
    ));
    recover_and_deliver(&runtime, &mut writes, AgentKind::Claude, text, false).await;
}

#[tokio::test]
async fn all_providers_recover_a_paste_rendered_after_the_uncertain_result() {
    for kind in [AgentKind::Claude, AgentKind::Codex, AgentKind::Cursor] {
        late_case(
            kind,
            "old room message",
            "old room message",
            None,
            false,
            false,
        )
        .await;
    }
}

#[tokio::test]
async fn a_partial_render_at_timeout_can_finish_and_recover_without_changing_recognition() {
    for kind in [AgentKind::Claude, AgentKind::Codex, AgentKind::Cursor] {
        late_case(
            kind,
            "old room message",
            "old room message",
            Some("old room"),
            false,
            false,
        )
        .await;
    }
}

#[tokio::test]
async fn slow_dispatch_and_late_provider_render_keep_the_old_paste_attributable() {
    for kind in [AgentKind::Claude, AgentKind::Codex, AgentKind::Cursor] {
        late_case(
            kind,
            "load delayed message",
            "load delayed message",
            None,
            false,
            true,
        )
        .await;
    }
}

#[tokio::test]
async fn long_wrapped_multiline_and_collapsed_late_pastes_are_recoverable() {
    let text = (0..4)
        .map(|n| format!("line {n} {}", "中text ".repeat(22)))
        .collect::<Vec<_>>()
        .join("\n");
    for kind in [AgentKind::Claude, AgentKind::Codex, AgentKind::Cursor] {
        late_case(kind, &text, &text, None, false, false).await;
        let chip = match kind {
            AgentKind::Claude => "[Pasted text #1 +3 lines]".into(),
            AgentKind::Cursor => "[Pasted text #1 +4 lines]".into(),
            _ => format!("[Pasted Content {} chars]", text.chars().count()),
        };
        late_case(kind, &text, &chip, None, true, false).await;
    }
}

#[tokio::test]
async fn late_image_attachments_and_literal_paths_do_not_block_the_next_message() {
    let text = "\"/tmp/bus-stale-image.png\"\nread this image";
    for kind in [AgentKind::Claude, AgentKind::Codex, AgentKind::Cursor] {
        let body = if kind == AgentKind::Cursor {
            text
        } else {
            "[Image #1] read this image"
        };
        late_case(kind, text, body, None, kind != AgentKind::Cursor, false).await;
    }
}

#[tokio::test]
async fn developer_typing_after_the_stale_paste_revokes_cleanup_ownership() {
    for kind in [AgentKind::Claude, AgentKind::Codex, AgentKind::Cursor] {
        let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(123, 48, 32);
        runtime.test_process_pty_bytes(b"\x1b[?2004h");
        show(&runtime, kind, "");
        let clock = runtime.test_submission_clock().unwrap();
        clock.pause();
        uncertain(&runtime, &mut writes, kind, "old message", None, false).await;
        show(&runtime, kind, "old message");
        runtime
            .try_send_developer_bytes(Bytes::from_static(b" human edit"))
            .unwrap();
        assert_eq!(writes.recv().await.unwrap().as_ref(), b" human edit");
        show(&runtime, kind, "old message human edit");
        clock.advance(Duration::from_secs(3));
        let error = send(&runtime, kind, NEXT, None)
            .recv_timeout(WAIT)
            .unwrap()
            .unwrap_err();
        assert!(crate::terminal::runtime::PromptNotShown::is(&error));
        assert!(
            writes.try_recv().is_err(),
            "never clear, stash or submit an edited draft"
        );
    }
}

#[tokio::test]
async fn a_render_that_completes_before_the_deadline_submits_normally() {
    for kind in [AgentKind::Claude, AgentKind::Codex, AgentKind::Cursor] {
        let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(123, 48, 32);
        runtime.test_process_pty_bytes(b"\x1b[?2004h");
        show(&runtime, kind, "");
        let clock = runtime.test_submission_clock().unwrap();
        clock.pause();
        let completion = send(&runtime, kind, "old message", None);
        let mut result =
            tokio::task::spawn_blocking(move || completion.recv_timeout(WAIT).unwrap());
        assert!(input_or_result(&mut writes, &mut result)
            .await
            .starts_with(b"\x1b[200~"));
        clock.wait_for_draft_wait();
        show(&runtime, kind, "old");
        clock.advance(Duration::from_millis(100));
        show(&runtime, kind, "old message");
        assert_eq!(
            input_or_result(&mut writes, &mut result).await.as_ref(),
            b"\r"
        );
        show(&runtime, kind, "");
        assert!(result.await.unwrap().is_ok());
    }
}

#[tokio::test]
async fn codex_late_paste_is_owned_across_coordinator_reconnect() {
    // A coordinator reconnect changes no server runtime or PTY: a fresh
    // submission/inspector below must use the record retained by that runtime.
    late_case(
        AgentKind::Codex,
        "old room message",
        "old room message",
        None,
        false,
        false,
    )
    .await;
}

#[tokio::test]
async fn cursor_late_paste_is_owned_across_coordinator_resume() {
    // Coordinator resume reconnects to this same server-side runtime; it
    // neither reconstructs the inspector nor serializes ownership on disk.
    late_case(
        AgentKind::Cursor,
        "old room message",
        "old room message",
        None,
        false,
        false,
    )
    .await;
}

#[tokio::test]
async fn server_restart_and_native_resume_do_not_reuse_the_dead_ptys_ownership() {
    for kind in [AgentKind::Claude, AgentKind::Codex, AgentKind::Cursor] {
        let (old, mut old_writes) = TerminalRuntime::test_with_channel_capacity(123, 48, 32);
        old.test_process_pty_bytes(b"\x1b[?2004h");
        show(&old, kind, "");
        old.test_submission_clock().unwrap().pause();
        uncertain(&old, &mut old_writes, kind, "old message", None, false).await;
        show(&old, kind, "old message");
        drop(old);
        drop(old_writes);
        // Real server native resume makes a new PTY, without history_ansi.
        // If a developer puts matching text in that new composer it is foreign.
        let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(123, 48, 32);
        runtime.test_process_pty_bytes(b"\x1b[?2004h");
        show(&runtime, kind, "old message");
        let error = send(&runtime, kind, NEXT, None)
            .recv_timeout(WAIT)
            .unwrap()
            .unwrap_err();
        assert!(crate::terminal::runtime::PromptNotShown::is(&error));
        assert!(writes.try_recv().is_err());
        show(&runtime, kind, "");
        runtime.test_submission_clock().unwrap().pause();
        let completion = send(&runtime, kind, NEXT, None);
        let mut result =
            tokio::task::spawn_blocking(move || completion.recv_timeout(WAIT).unwrap());
        assert!(input_or_result(&mut writes, &mut result)
            .await
            .starts_with(b"\x1b[200~"));
        show(&runtime, kind, NEXT);
        assert_eq!(
            input_or_result(&mut writes, &mut result).await.as_ref(),
            b"\r"
        );
        show(&runtime, kind, "");
        assert!(result.await.unwrap().is_ok());
    }
}

#[tokio::test]
async fn agent_relaunch_invalidates_ownership_even_when_the_terminal_runtime_is_reused() {
    for kind in [AgentKind::Claude, AgentKind::Codex, AgentKind::Cursor] {
        let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(123, 48, 32);
        runtime.test_process_pty_bytes(b"\x1b[?2004h");
        show(&runtime, kind, "");
        runtime.test_submission_clock().unwrap().pause();
        uncertain(&runtime, &mut writes, kind, "old message", None, false).await;
        runtime.mark_agent_launch();
        show(&runtime, kind, "old message");
        let error = send(&runtime, kind, NEXT, None)
            .recv_timeout(WAIT)
            .unwrap()
            .unwrap_err();
        assert!(crate::terminal::runtime::PromptNotShown::is(&error));
        assert!(
            writes.try_recv().is_err(),
            "a new agent's draft is not the old paste"
        );
    }
}

#[path = "late_wait_test.rs"]
mod wait_tests;
