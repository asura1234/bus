//! Sanitized Claude Code 2.1.316 transcript and editable composer fixtures.
use super::*;
use crate::agents::providers::claude_code::retirement::TranscriptAnchor;
use std::{fs, io::Write};

const PROMPT: &str = r#"{"type":"user","uuid":"original","parentUuid":null,"promptId":"prompt","sessionId":"session","version":"2.1.316","isSidechain":false,"promptSource":"typed","turnOrigin":"human","message":{"role":"user","content":"from bus"}}"#;
const TOOL: &str = r#"{"type":"assistant","uuid":"tool","parentUuid":"original","sessionId":"session","version":"2.1.316","isSidechain":false,"message":{"content":[{"type":"tool_use","id":"tool-id","name":"Bash","input":{}}]}}"#;
const RESULT: &str = r#"{"type":"user","uuid":"result","parentUuid":"tool","promptId":"prompt","sessionId":"session","version":"2.1.316","isSidechain":false,"sourceToolAssistantUUID":"tool","message":{"content":[{"type":"tool_result","tool_use_id":"tool-id","content":"done"}]}}"#;
const STREAM: &str = r#"{"type":"assistant","uuid":"thinking","parentUuid":"result","sessionId":"session","version":"2.1.316","isSidechain":false,"message":{"content":[{"type":"thinking","thinking":"continuing"}]}}"#;
const CANCEL: &str = r#"{"type":"user","uuid":"cancel","parentUuid":"thinking","promptId":"prompt","sessionId":"session","version":"2.1.316","isSidechain":false,"interruptedMessageId":"11111111-2222-4333-8444-555555555555","message":{"content":[{"type":"text","text":"[Request interrupted by user]"}]}}"#;

fn interrupted_frame(runtime: &TerminalRuntime, body: &str, bottom: &str, footer: &str) {
    runtime.test_process_pty_bytes(
        format!(
            "\x1b[2J\x1b[HInterrupted · What should Claude do instead?\r\n────────────────\r\n❯ {body}\r\n{bottom}\r\n{footer}\x1b[3;3H"
        )
        .as_bytes(),
    );
}

// The live Claude 2.1.316 producer's cancelled API message ID was absent
// from earlier transcript identifiers, including the parent assistant row.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EarlyEvidence {
    None,
    Cancel,
    Frame,
}

async fn exercise(
    before: &str,
    after: &str,
    body: &str,
    bottom: &str,
    footer: &str,
    qualifies: bool,
    restores_late: bool,
    early: EarlyEvidence,
) {
    let directory = crate::utils::test_temp::unique_temp_path("claude-no-restoration");
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("session.jsonl");
    fs::write(&path, before).unwrap();
    let anchor = TranscriptAnchor::from_hook(&path, "session", "prompt").unwrap();
    let receipt = directory.join("completion");
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    confirmed_plain(&runtime, &mut writes, &directory, "from bus").await;
    let clock = runtime.test_submission_clock().unwrap();
    clock.pause();
    let generation = clock.advance(Duration::ZERO);
    let delivery = (early != EarlyEvidence::None).then(|| clock.hold_next_write_delivery());
    let sender = runtime.submission_handle(true);
    let original = receipt.clone();
    let stop = tokio::task::spawn_blocking(move || {
        sender.retire_claude_interrupt_with_anchor(
            "session",
            &["from bus".into()],
            Bytes::from_static(b"\x1b"),
            Some(&original),
            Some(&anchor),
        )
    });
    let append = || {
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(after.as_bytes())
            .unwrap();
    };
    if let Some((waiting, release)) = delivery {
        waiting.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            writes.try_recv().is_err(),
            "Esc has not reached the fake PTY"
        );
        if early == EarlyEvidence::Cancel {
            append();
        } else {
            interrupted_frame(&runtime, body, bottom, footer);
        }
        release.send(()).unwrap();
    }
    assert_eq!(key(&mut writes).await.as_ref(), b"\x1b");
    clock.wait_for_submission_check(generation);
    clock.resume();
    if early != EarlyEvidence::Cancel {
        append();
    }
    if early != EarlyEvidence::Frame {
        interrupted_frame(&runtime, body, bottom, footer);
    }
    if restores_late {
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(
            !stop.is_finished(),
            "empty must wait through the retirement budget"
        );
        assert!(writes.try_recv().is_err());
        paint(&runtime, "from bus", false, "late restoration");
        clear_restored(&runtime, &mut writes, "from bus").await;
    }
    assert_eq!(stop.await.unwrap().is_ok(), qualifies);
    assert_eq!(receipt.exists(), qualifies);
    assert!(writes.try_recv().is_err(), "Esc was the only key");
    if qualifies {
        let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
        let sender = runtime.submission_handle(true);
        sender.bind_claude_session("session");
        sender
            .retire_claude_interrupt_with_anchor(
                "session",
                &["from bus".into()],
                Bytes::from_static(b"\x1b"),
                Some(&receipt),
                None,
            )
            .unwrap();
        assert!(
            writes.try_recv().is_err(),
            "receipt retry never repeats Esc"
        );
    }
}

#[tokio::test]
async fn claude_tool_progress_stream_cancel_retires_without_editing_and_reuses_receipt() {
    exercise(
        &format!("{PROMPT}\n{TOOL}\n{RESULT}\n"),
        &format!("{STREAM}\n{CANCEL}\n"),
        "",
        "────────────────",
        "",
        true,
        false,
        EarlyEvidence::None,
    )
    .await;
}

#[tokio::test]
async fn claude_no_restoration_holds_without_fresh_linked_tool_and_cancel_evidence() {
    let before = format!("{PROMPT}\n{TOOL}\n{RESULT}\n");
    let after = format!("{STREAM}\n{CANCEL}\n");
    let no_progress = CANCEL.replace(r#""parentUuid":"thinking""#, r#""parentUuid":"original""#);
    let cases = [
        (format!("{PROMPT}\n"), format!("{no_progress}\n")),
        (format!("{before}{after}"), String::new()),
        (
            before.clone(),
            after.replace(r#""promptId":"prompt""#, r#""promptId":"other""#),
        ),
        (
            before.clone(),
            after.replace(r#""sessionId":"session""#, r#""sessionId":"other""#),
        ),
        (
            before.clone(),
            after.replace(
                r#""interruptedMessageId":"11111111-2222-4333-8444-555555555555","#,
                "",
            ),
        ),
        (before, after.trim_end().to_owned()),
    ];
    for (before, after) in cases {
        exercise(
            &before,
            &after,
            "",
            "────────────────",
            "",
            false,
            false,
            EarlyEvidence::None,
        )
        .await;
    }
}

#[tokio::test]
async fn claude_no_restoration_requires_complete_empty_composer() {
    let before = format!("{PROMPT}\n{TOOL}\n{RESULT}\n");
    let after = format!("{STREAM}\n{CANCEL}\n");
    for (body, bottom, footer) in [
        ("", "", ""),
        ("", "────────────────", "› stashed"),
        ("\x1b[2mhidden\x1b[0m", "────────────────", ""),
        ("foreign edit", "────────────────", ""),
        (
            "",
            "────────────────",
            "❯ [ ] Alpha\r\n  [ ] Beta\r\nSpace:select",
        ),
    ] {
        exercise(
            &before,
            &after,
            body,
            bottom,
            footer,
            false,
            false,
            EarlyEvidence::None,
        )
        .await;
    }
}

#[tokio::test]
async fn claude_late_restoration_takes_precedence_over_typed_cancel_evidence() {
    exercise(
        &format!("{PROMPT}\n{TOOL}\n{RESULT}\n"),
        &format!("{STREAM}\n{CANCEL}\n"),
        "",
        "────────────────",
        "",
        true,
        true,
        EarlyEvidence::None,
    )
    .await;
}

#[tokio::test]
async fn claude_typed_cancel_cannot_retire_an_opaque_or_joined_receipt() {
    for opaque in [false, true] {
        let directory =
            crate::utils::test_temp::unique_temp_path("claude-no-restoration-restricted");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("session.jsonl");
        fs::write(&path, format!("{PROMPT}\n{TOOL}\n{RESULT}\n")).unwrap();
        let anchor = TranscriptAnchor::from_hook(&path, "session", "prompt").unwrap();
        let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
        let payload = if opaque {
            "[Pasted text #2 +3 lines]"
        } else {
            "from bus"
        };
        confirmed_plain(&runtime, &mut writes, &directory, payload).await;
        let payloads = if opaque {
            vec![payload.into()]
        } else {
            vec![payload.into(), "joined input".into()]
        };
        let sender = runtime.submission_handle(true);
        let stop = tokio::task::spawn_blocking(move || {
            sender.retire_claude_interrupt_with_anchor(
                "session",
                &payloads,
                Bytes::from_static(b"\x1b"),
                None,
                Some(&anchor),
            )
        });
        if !opaque {
            assert_eq!(key(&mut writes).await.as_ref(), b"\x1b");
            let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
            writeln!(file, "{STREAM}\n{CANCEL}").unwrap();
            interrupted_frame(&runtime, "", "────────────────", "");
        }
        assert!(stop.await.unwrap().is_err());
        assert!(
            writes.try_recv().is_err(),
            "no editing keys on joined or opaque input"
        );
    }
}

async fn reject(before: String, after: String, early: EarlyEvidence) {
    exercise(
        &before,
        &after,
        "",
        "────────────────",
        "",
        false,
        false,
        early,
    )
    .await;
}

#[tokio::test]
async fn claude_no_restoration_rejects_a_pre_stop_cancel_uuid_appended_again() {
    reject(
        format!("{PROMPT}\n{TOOL}\n{RESULT}\n{STREAM}\n{CANCEL}\n"),
        format!("{CANCEL}\n"),
        EarlyEvidence::None,
    )
    .await;
    // Even a rewritten row cannot reuse a cancellation UUID from far outside
    // the parent-walk tail of the same transcript.
    let earlier = CANCEL.replace("11111111-2222-4333-8444-555555555555", "older-message");
    let padding = serde_json::json!({"type":"api-request-blob","blob":"x".repeat(2 * 1024 * 1024)});
    reject(
        format!("{earlier}\n{padding}\n{PROMPT}\n{TOOL}\n{RESULT}\n"),
        format!("{STREAM}\n{CANCEL}\n"),
        EarlyEvidence::None,
    )
    .await;
}

#[tokio::test]
async fn claude_no_restoration_rejects_duplicate_cancel_rows_after_delivery() {
    reject(
        format!("{PROMPT}\n{TOOL}\n{RESULT}\n"),
        format!("{STREAM}\n{CANCEL}\n{CANCEL}\n"),
        EarlyEvidence::None,
    )
    .await;
}

#[tokio::test]
async fn claude_no_restoration_rejects_cancel_before_esc_delivery() {
    reject(
        format!("{PROMPT}\n{TOOL}\n{RESULT}\n"),
        format!("{STREAM}\n{CANCEL}\n"),
        EarlyEvidence::Cancel,
    )
    .await;
}

#[tokio::test]
async fn claude_no_restoration_rejects_empty_frame_before_esc_delivery() {
    reject(
        format!("{PROMPT}\n{TOOL}\n{RESULT}\n"),
        format!("{STREAM}\n{CANCEL}\n"),
        EarlyEvidence::Frame,
    )
    .await;
}

#[tokio::test]
async fn claude_no_restoration_rejects_a_reused_or_off_chain_api_message() {
    let off_chain = r#"{"type":"assistant","uuid":"other-branch","parentUuid":"foreign","sessionId":"session","version":"2.1.316","isSidechain":false,"message":{"id":"11111111-2222-4333-8444-555555555555"}}"#;
    let earlier_cancel = CANCEL.replace(r#""uuid":"cancel""#, r#""uuid":"other-cancel""#);
    for row in [off_chain, &earlier_cancel] {
        reject(
            format!("{PROMPT}\n{TOOL}\n{RESULT}\n{row}\n"),
            format!("{STREAM}\n{CANCEL}\n"),
            EarlyEvidence::None,
        )
        .await;
    }
}
