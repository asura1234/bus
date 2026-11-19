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
    runtime.test_process_pty_bytes(interrupted_frame_bytes(body, bottom, footer).as_bytes());
}

fn interrupted_frame_bytes(body: &str, bottom: &str, footer: &str) -> String {
    format!(
        "\x1b[2J\x1b[HInterrupted · What should Claude do instead?\r\n────────────────\r\n❯ {body}\r\n{bottom}\r\n{footer}\x1b[3;3H"
    )
}

// The live Claude 2.1.316 producer's cancelled API message ID was absent
// from earlier transcript identifiers, including the parent assistant row.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EarlyEvidence {
    None,
    Cancel,
    ToolBeforeDelivery,
    Frame,
    InFlightFrame,
    Scroll,
    SynchronizedFrame,
    LiveChrome,
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
    let delivery = (!matches!(early, EarlyEvidence::None | EarlyEvidence::LiveChrome))
        .then(|| clock.hold_next_write_delivery());
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
    let append = |rows: &str| {
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(rows.as_bytes())
            .unwrap();
    };
    if let Some((waiting, release)) = delivery {
        waiting.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            writes.try_recv().is_err(),
            "Esc has not reached the fake PTY"
        );
        if early == EarlyEvidence::InFlightFrame {
            let frame = interrupted_frame_bytes(body, bottom, footer);
            runtime.test_process_pty_bytes_with_hook(frame.as_bytes(), || {
                release.send(()).unwrap();
                clock.wait_for_submission_check(generation);
            });
        } else {
            if early == EarlyEvidence::Cancel {
                append(after);
            } else if early == EarlyEvidence::ToolBeforeDelivery {
                append(after.split_inclusive('\n').next().unwrap());
            } else {
                if early == EarlyEvidence::SynchronizedFrame {
                    runtime.test_process_pty_bytes(b"\x1b[?2026h");
                }
                interrupted_frame(&runtime, body, bottom, footer);
                if early == EarlyEvidence::SynchronizedFrame {
                    assert!(runtime.synchronized_output_active());
                }
            }
            release.send(()).unwrap();
        }
    }
    assert_eq!(key(&mut writes).await.as_ref(), b"\x1b");
    clock.wait_for_submission_check(generation);
    clock.resume();
    match early {
        EarlyEvidence::Cancel => {}
        EarlyEvidence::ToolBeforeDelivery => append(after.split_once('\n').unwrap().1),
        _ => append(after),
    }
    match early {
        EarlyEvidence::None | EarlyEvidence::Cancel | EarlyEvidence::ToolBeforeDelivery => {
            interrupted_frame(&runtime, body, bottom, footer);
        }
        EarlyEvidence::LiveChrome => {
            let frame = interrupted_frame_bytes(body, bottom, footer).replace("❯ ", "❯\u{a0}");
            runtime.test_process_pty_bytes(frame.as_bytes());
        }
        EarlyEvidence::Scroll => {
            runtime.scroll_up(1);
            runtime.scroll_reset();
        }
        EarlyEvidence::SynchronizedFrame => runtime.test_process_pty_bytes(b"\x1b[?2026l"),
        EarlyEvidence::Frame | EarlyEvidence::InFlightFrame => {}
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

#[tokio::test]
async fn claude_no_restoration_rejects_a_pty_batch_in_flight_at_the_stop_baseline() {
    reject(
        format!("{PROMPT}\n{TOOL}\n{RESULT}\n"),
        format!("{STREAM}\n{CANCEL}\n"),
        EarlyEvidence::InFlightFrame,
    )
    .await;
}

#[tokio::test]
async fn claude_no_restoration_rejects_scroll_without_later_pty_output() {
    reject(
        format!("{PROMPT}\n{TOOL}\n{RESULT}\n"),
        format!("{STREAM}\n{CANCEL}\n"),
        EarlyEvidence::Scroll,
    )
    .await;
}

#[tokio::test]
async fn claude_no_restoration_rejects_completion_of_a_pre_stop_synchronized_frame() {
    reject(
        format!("{PROMPT}\n{TOOL}\n{RESULT}\n"),
        format!("{STREAM}\n{CANCEL}\n"),
        EarlyEvidence::SynchronizedFrame,
    )
    .await;
}

#[tokio::test]
async fn claude_no_restoration_rejects_nonunique_parent_and_anchor_uuids() {
    let wrong_parent = STREAM.replace(r#""parentUuid":"result""#, r#""parentUuid":"foreign""#);
    let wrong_anchor = PROMPT.replace(r#""parentUuid":null"#, r#""parentUuid":"foreign""#);
    let large_metadata =
        serde_json::json!({"type":"api-request-blob","blob":"x".repeat(2 * 1024 * 1024)});
    for older in [
        wrong_parent.clone(),
        wrong_anchor,
        format!("{wrong_parent}\n{large_metadata}"),
    ] {
        reject(
            format!("{older}\n{PROMPT}\n{TOOL}\n{RESULT}\n"),
            format!("{STREAM}\n{CANCEL}\n"),
            EarlyEvidence::None,
        )
        .await;
    }
}

#[tokio::test]
async fn claude_no_restoration_rejects_a_branch_in_the_anchored_parent_chain() {
    let branch = STREAM.replace(r#""uuid":"thinking""#, r#""uuid":"sibling""#);
    let mut missing_uuid: serde_json::Value = serde_json::from_str(&branch).unwrap();
    missing_uuid.as_object_mut().unwrap().remove("uuid");
    for extra in [missing_uuid.to_string(), branch] {
        reject(
            format!("{PROMPT}\n{TOOL}\n{RESULT}\n{extra}\n"),
            format!("{STREAM}\n{CANCEL}\n"),
            EarlyEvidence::None,
        )
        .await;
    }
}

#[tokio::test]
async fn claude_ds_completed_tool_progress_retires_the_live_nbsp_composer() {
    let attachment = r#"{"type":"attachment","uuid":"metadata","parentUuid":"result","sessionId":"session","version":"2.1.316","isSidechain":false}"#;
    let stream = STREAM.replace(r#""parentUuid":"result""#, r#""parentUuid":"metadata""#);
    exercise(
        &format!("{PROMPT}\n{TOOL}\n{RESULT}\n{attachment}\n"),
        &format!("{stream}\n{CANCEL}\n"),
        "",
        "────────────────",
        "",
        true,
        false,
        EarlyEvidence::LiveChrome,
    )
    .await;
}

const CANCELLED_STREAM: &str = "11111111-2222-4333-8444-555555555555";

fn with_stream_id(row: &str) -> String {
    let mut value: serde_json::Value = serde_json::from_str(row).unwrap();
    value["message"]["id"] = serde_json::json!(CANCELLED_STREAM);
    value.to_string()
}

#[tokio::test]
async fn claude_ds_cancelled_first_tool_retires_its_linked_api_stream() {
    let thinking = with_stream_id(
        &STREAM
            .replace(r#""uuid":"thinking""#, r#""uuid":"initial-thinking""#)
            .replace(r#""parentUuid":"result""#, r#""parentUuid":"original""#),
    );
    let tool = with_stream_id(&TOOL.replace(
        r#""parentUuid":"original""#,
        r#""parentUuid":"initial-thinking""#,
    ));
    let cancelled_result = RESULT.replace(
        r#""type":"tool_result""#,
        r#""type":"tool_result","is_error":true"#,
    );
    for bridge in [false, true] {
        let parent = if bridge { "result" } else { "tool" };
        let cancel = CANCEL.replace(
            r#""parentUuid":"thinking""#,
            &format!(r#""parentUuid":"{parent}""#),
        );
        let after = if bridge {
            format!("{cancelled_result}\n{cancel}\n")
        } else {
            format!("{cancel}\n")
        };
        exercise(
            &format!("{PROMPT}\n{thinking}\n{tool}\n"),
            &after,
            "",
            "────────────────",
            "",
            true,
            false,
            EarlyEvidence::None,
        )
        .await;
    }
}

#[tokio::test]
async fn claude_linked_api_id_requires_the_cancelled_streams_own_pre_stop_tool() {
    let matching_tool = with_stream_id(TOOL);
    let thinking =
        with_stream_id(&STREAM.replace(r#""parentUuid":"result""#, r#""parentUuid":"original""#));
    let mixed = STREAM.replace(r#""parentUuid":"result""#, r#""parentUuid":"tool""#);
    let prior_foreign = thinking.replace(CANCELLED_STREAM, "foreign-stream");
    let tool_after_thinking =
        matching_tool.replace(r#""parentUuid":"original""#, r#""parentUuid":"thinking""#);
    let cancelled_result = RESULT.replace(
        r#""type":"tool_result""#,
        r#""type":"tool_result","is_error":true"#,
    );
    let wrong_bridge =
        cancelled_result.replace(r#""tool_use_id":"tool-id""#, r#""tool_use_id":"foreign""#);
    let cancel_tool = CANCEL.replace(r#""parentUuid":"thinking""#, r#""parentUuid":"tool""#);
    let cancel_result = CANCEL.replace(r#""parentUuid":"thinking""#, r#""parentUuid":"result""#);
    for (before, after) in [
        (PROMPT.to_owned(), format!("{matching_tool}\n{cancel_tool}")),
        (format!("{PROMPT}\n{thinking}"), CANCEL.to_owned()),
        (
            format!("{PROMPT}\n{matching_tool}"),
            format!("{mixed}\n{CANCEL}"),
        ),
        (
            format!("{PROMPT}\n{prior_foreign}\n{tool_after_thinking}"),
            cancel_tool.clone(),
        ),
        (
            format!("{PROMPT}\n{matching_tool}"),
            format!("{wrong_bridge}\n{cancel_result}"),
        ),
        (
            format!("{PROMPT}\n{matching_tool}\n{cancelled_result}"),
            cancel_result,
        ),
    ] {
        reject(
            format!("{before}\n"),
            format!("{after}\n"),
            EarlyEvidence::None,
        )
        .await;
    }
    reject(
        format!("{PROMPT}\n"),
        format!("{matching_tool}\n{cancel_tool}\n"),
        EarlyEvidence::ToolBeforeDelivery,
    )
    .await;
}
