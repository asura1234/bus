use super::*;
use serde_json::json;
use std::path::PathBuf;

fn token_count(limits: Value) -> String {
    json!({"timestamp":"2026-10-05T00:00:00Z","type":"event_msg",
        "payload":{"type":"token_count","info":null,"rate_limits":limits}})
    .to_string()
}

#[test]
fn rollout_uses_newest_rate_limits_and_classifies_windows_by_duration() {
    let dir = crate::utils::test_temp::unique_temp_path("bus-usage");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rollout-2026-10-05T00-00-00-abc.jsonl");
    let lines = [
        token_count(json!({"primary":{"used_percent":90.0,"window_minutes":300,"resets_at":1}})),
        // Weekly in primary, five-hour in secondary: slots are not stable.
        token_count(json!({
            "limit_id":"codex",
            "primary":{"used_percent":12.5,"window_minutes":10080,"resets_at":1791788174},
            "secondary":{"used_percent":40.0,"window_minutes":300,"resets_at":1791700000},
        })),
        // Newer lines without rate limits do not hide the last known values.
        token_count(Value::Null),
        json!({"type":"response_item","payload":{"type":"message"}}).to_string(),
    ];
    std::fs::write(&path, format!("{{\"partial\n{}\n", lines.join("\n"))).unwrap();

    let windows = read_codex_rollout(&path).unwrap().unwrap();
    assert_eq!(
        windows.weekly,
        Some(UsageWindow {
            used_percent: 12.5,
            resets_at: Some(1791788174),
            window_minutes: 10080
        })
    );
    assert_eq!(windows.five_hour.unwrap().used_percent, 40.0);

    std::fs::write(
        &path,
        token_count(
            json!({"primary":{"used_percent":3.0,"window_minutes":10080},"secondary":null}),
        ),
    )
    .unwrap();
    let windows = read_codex_rollout(&path).unwrap().unwrap();
    assert_eq!(windows.five_hour, None, "absent window stays unknown");
    assert_eq!(windows.weekly.unwrap().resets_at, None);

    std::fs::write(&path, "not json\n").unwrap();
    assert_eq!(read_codex_rollout(&path).unwrap(), None);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn only_absolute_codex_rollout_paths_are_read() {
    let accepted =
        json!({"transcript_path":"/Users/me/.codex/sessions/2026/10/05/rollout-1.jsonl"});
    assert!(codex_rollout_path(&accepted).is_some());
    for path in [
        json!("relative/rollout-1.jsonl"),
        json!("/Users/me/.ssh/id_ed25519"),
        json!("/tmp/rollout-1.json"),
        Value::Null,
    ] {
        assert!(
            codex_rollout_path(&json!({"transcript_path":path})).is_none(),
            "{path}"
        );
    }
}

fn task_complete(turn: &str, error: Value) -> String {
    json!({"timestamp":"2026-10-12T13:02:52.716Z","type":"event_msg",
        "payload":{"type":"task_complete","turn_id":turn,"last_agent_message":null,"error":error}})
    .to_string()
}

#[test]
fn rollout_reports_the_error_that_ended_a_given_codex_turn() {
    let dir = crate::utils::test_temp::unique_temp_path("bus-turn-error");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rollout-2026-10-12T00-00-00-abc.jsonl");
    let capacity = json!({"message":"Selected model is at capacity. Please try a different model.",
        "codex_error_info":"server_overloaded"});
    let lines = [
        // A partial first line, as after the tail seek, is skipped.
        "\"payload\":{\"type\":\"task_complete\"".to_owned(),
        task_complete("turn-1", capacity.clone()),
        task_complete("turn-2", Value::Null),
        task_complete("turn-3", capacity),
    ];
    std::fs::write(&path, lines.join("\n")).unwrap();
    assert_eq!(
        read_codex_turn_end(&path, "turn-1").unwrap(),
        Some(CodexTurnEnd::Error(
            "Selected model is at capacity. Please try a different model. (server_overloaded)"
                .into()
        ))
    );
    // A committed textless end differs from an absent or still-running turn.
    assert_eq!(
        read_codex_turn_end(&path, "turn-2").unwrap(),
        Some(CodexTurnEnd::NoReply)
    );
    assert_eq!(read_codex_turn_end(&path, "turn-9").unwrap(), None);
    assert!(read_codex_turn_end(&dir.join("missing.jsonl"), "turn-1").is_err());
    std::fs::remove_dir_all(dir).unwrap();
}

fn rollout_dir(name: &str) -> PathBuf {
    let dir = crate::utils::test_temp::unique_temp_path(&format!("bus-{name}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Event shapes measured from a Codex 0.163.0 rollout (2026-10-20): the
/// prompt is a `UserMessage` item of its turn, the answer the turn's
/// `task_complete.last_agent_message`.
fn codex_turn(turn: &str, at: &str, prompt: &str, answer: Option<&str>) -> Vec<String> {
    let mut lines = vec![
        json!({"timestamp":at,"type":"event_msg","payload":{"type":"task_started","turn_id":turn}}),
        json!({"timestamp":at,"type":"response_item","payload":{"type":"message","role":"user",
            "content":[{"type":"input_text","text":prompt}]}}),
        json!({"timestamp":at,"type":"event_msg","payload":{"type":"item_completed","turn_id":turn,
            "item":{"type":"UserMessage","content":[{"type":"text","text":prompt}]}}}),
    ];
    if let Some(answer) = answer {
        lines.push(
            json!({"timestamp":at,"type":"event_msg","payload":{"type":"task_complete",
            "turn_id":turn,"last_agent_message":answer}}),
        );
    }
    lines.iter().map(Value::to_string).collect()
}

#[test]
fn rollout_reports_the_final_answer_that_ended_a_given_codex_turn() {
    let dir = rollout_dir("turn-final");
    let path = dir.join("rollout-2026-10-20T00-00-00-abc.jsonl");
    let mut lines = codex_turn(
        "turn-1",
        "2026-10-20T05:06:51.361Z",
        "first",
        Some("Merged."),
    );
    lines.extend(codex_turn(
        "turn-2",
        "2026-10-20T05:54:12.249Z",
        "second",
        None,
    ));
    std::fs::write(&path, lines.join("\n")).unwrap();
    assert_eq!(
        read_codex_turn_end(&path, "turn-1").unwrap(),
        Some(CodexTurnEnd::Final("Merged.".into()))
    );
    // A turn still running, or one never recorded, has not ended.
    assert_eq!(read_codex_turn_end(&path, "turn-2").unwrap(), None);
    assert_eq!(read_codex_turn_end(&path, "turn-9").unwrap(), None);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rollout_finds_the_one_turn_that_ran_a_prompt_after_it_was_sent() {
    let dir = rollout_dir("prompt-turn");
    let path = dir.join("rollout-2026-10-20T00-00-00-abc.jsonl");
    let mut lines = codex_turn("turn-1", "2026-10-20T05:06:51.361Z", "status?", Some("a"));
    lines.extend(codex_turn(
        "turn-2",
        "2026-10-20T05:54:12.249Z",
        "close it",
        Some("b"),
    ));
    lines.extend(codex_turn(
        "turn-3",
        "2026-10-20T05:56:00.000Z",
        "status?",
        Some("c"),
    ));
    std::fs::write(&path, lines.join("\n")).unwrap();
    let sent = |at: &str| {
        let at = time::OffsetDateTime::parse(at, &time::format_description::well_known::Rfc3339)
            .unwrap();
        u64::try_from(at.unix_timestamp_nanos() / 1_000_000).unwrap()
    };
    let turn = |prompt: &str, since: &str| {
        find_codex_prompt_turn(&path, sent(since), |text| text == prompt).unwrap()
    };
    assert_eq!(
        turn("close it", "2026-10-20T05:54:12.172Z"),
        Some("turn-2".into())
    );
    // Only a turn started after the message was sent can be its turn.
    assert_eq!(turn("close it", "2026-10-20T05:54:13.000Z"), None);
    // Two turns that ran the same prompt since it was sent are ambiguous.
    assert_eq!(turn("status?", "2026-10-20T05:00:00.000Z"), None);
    assert_eq!(
        turn("status?", "2026-10-20T05:55:00.000Z"),
        Some("turn-3".into())
    );
    assert_eq!(turn("unrelated", "2026-10-20T05:00:00.000Z"), None);
    assert!(find_codex_prompt_turn(&dir.join("missing.jsonl"), 0, |_| true).is_err());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rollout_prompt_search_sees_every_turn_since_the_send_however_long() {
    let dir = rollout_dir("prompt-window");
    let path = dir.join("rollout-2026-10-20T00-00-00-abc.jsonl");
    let mut lines = codex_turn("turn-0", "2026-10-20T05:00:00.000Z", "older", Some("o"));
    lines.extend(codex_turn(
        "turn-1",
        "2026-10-20T05:06:51.361Z",
        "status?",
        None,
    ));
    // A turn's reasoning and tool output can run to megabytes.
    let filler = json!({"timestamp":"2026-10-20T05:07:00.000Z","type":"response_item",
        "payload":{"type":"reasoning","encrypted_content":"x".repeat(300 * 1024)}});
    lines.push(filler.to_string());
    lines.extend(codex_turn(
        "turn-2",
        "2026-10-20T05:54:12.249Z",
        "status?",
        Some("b"),
    ));
    std::fs::write(&path, lines.join("\n")).unwrap();
    let since = 1_792_472_500_000; // 2026-10-20T05:01:40Z
                                   // The earlier matching turn lies past the last 256 KiB, yet still counts.
    assert_eq!(
        find_codex_prompt_turn(&path, since, |text| text == "status?").unwrap(),
        None
    );
    assert_eq!(
        find_codex_prompt_turn(&path, since, |text| text == "older").unwrap(),
        None
    );
    // A window longer than Bus will read is never searched in part.
    let cap = 64 * 1024;
    assert_eq!(
        find_prompt_turn_within(&path, since, |text| text == "status?", cap).unwrap(),
        None
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rollout_prompt_search_refuses_a_window_longer_than_it_reads() {
    let dir = rollout_dir("prompt-cap");
    let path = dir.join("rollout-2026-10-20T00-00-00-abc.jsonl");
    let mut lines = codex_turn("turn-1", "2026-10-20T05:06:51.361Z", "close it", Some("a"));
    let filler = json!({"timestamp":"2026-10-20T05:07:00.000Z","type":"response_item",
        "payload":{"type":"reasoning","encrypted_content":"x".repeat(100 * 1024)}});
    lines.push(filler.to_string());
    std::fs::write(&path, lines.join("\n")).unwrap();
    let since = 1_792_472_800_000; // 2026-10-20T05:06:40Z
    let find = |cap| find_prompt_turn_within(&path, since, |text| text == "close it", cap);
    assert_eq!(find(1024 * 1024).unwrap(), Some("turn-1".into()));
    // Bus cannot see where the window starts, so it attributes nothing.
    assert_eq!(find(64 * 1024).unwrap(), None);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rollout_prompt_search_says_why_it_attributes_nothing() {
    let dir = rollout_dir("prompt-outcome");
    let path = dir.join("rollout-2026-10-20T00-00-00-abc.jsonl");
    let mut lines = codex_turn("turn-1", "2026-10-20T05:06:51.361Z", "status?", Some("a"));
    lines.extend(codex_turn(
        "turn-2",
        "2026-10-20T05:06:59.000Z",
        "status?",
        Some("b"),
    ));
    lines.extend(codex_turn(
        "turn-3",
        "2026-10-20T05:07:30.000Z",
        "close it",
        Some("c"),
    ));
    std::fs::write(&path, lines.join("\n")).unwrap();
    let since = 1_792_472_820_000; // 2026-10-20T05:07:00Z
    let outcome = |prompt: &str| codex_prompt_turn(&path, since, |text| text == prompt).unwrap();
    assert_eq!(outcome("close it"), PromptTurn::Found("turn-3".into()));
    assert_eq!(outcome("unrelated"), PromptTurn::NoMatch);
    // Both earlier turns lie within the clock margin before the send.
    assert_eq!(outcome("status?"), PromptTurn::Ambiguous);
    let late = 1_792_472_870_000; // 2026-10-20T05:07:50Z
    let late_outcome = codex_prompt_turn(&path, late, |text| text == "close it").unwrap();
    assert_eq!(late_outcome, PromptTurn::StartedBeforeSend);
    assert_eq!(
        prompt_turn_within(&path, since, |text| text == "close it", 64).unwrap(),
        PromptTurn::WindowTooLong
    );
    assert_eq!(
        codex_turn_prompts(&path, "turn-2", since - 60_000).unwrap(),
        vec!["status?".to_owned()]
    );
    std::fs::remove_dir_all(dir).unwrap();
}

const FINAL_0164: &str = include_str!("fixtures/final-answer-0.164.0.jsonl");

#[test]
fn rollout_0164_recovers_the_actual_four_lost_finals() {
    let dir = rollout_dir("0164-lost-finals");
    let path = dir.join("rollout-recorded.jsonl");
    std::fs::write(&path, FINAL_0164).unwrap();
    for turn in [
        "01a19a4c-21df-7ba1-93ea-4e073e8af0e0",
        "01a19a5b-fdfc-7682-955d-bb17d87142ff",
        "01a19a71-3f41-7a62-aba0-76c60d69c99c",
        "01a19a73-1b61-7ca1-83c3-106c97f20b45",
    ] {
        assert_eq!(
            read_codex_turn_end(&path, turn).unwrap(),
            Some(CodexTurnEnd::Final("".into())),
            "{turn}"
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}

fn final_0164_records() -> Vec<Value> {
    FINAL_0164
        .lines()
        .take(4)
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn rollout_0164_reads_completed_message_items_and_preserves_text_parts() {
    let dir = rollout_dir("0164-items");
    let path = dir.join("rollout-recorded.jsonl");
    let turn = "01a19a4c-21df-7ba1-93ea-4e073e8af0e0";
    for response in [false, true] {
        let mut lines = final_0164_records();
        lines[1]["payload"]["item"]["content"] = json!([
            {"type":"Text","text":"exact "}, {"type":"Text","text":"answer"}]);
        lines[2]["payload"]["content"] = json!([
            {"type":"output_text","text":"exact "}, {"type":"output_text","text":"answer"}]);
        lines.remove(if response { 1 } else { 2 });
        std::fs::write(
            &path,
            lines
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        assert_eq!(
            read_codex_turn_end(&path, turn).unwrap(),
            Some(CodexTurnEnd::Final("exact answer".into()))
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn noise_rollout_empty_final_preserves_text_from_each_committed_codex_shape() {
    let dir = rollout_dir("empty-final");
    let path = dir.join("rollout-recorded.jsonl");
    let turn = "01a19a4c-21df-7ba1-93ea-4e073e8af0e0";
    for text in ["", " \t\n"] {
        for source in ["task_complete", "item_completed", "response_item"] {
            let mut lines = final_0164_records();
            match source {
                "task_complete" => {
                    lines[3]["payload"]["last_agent_message"] = json!(text);
                    lines.drain(1..3);
                }
                "item_completed" => {
                    lines[1]["payload"]["item"]["content"] = json!([{"type":"Text","text":text}]);
                    lines.remove(2);
                }
                "response_item" => {
                    lines[2]["payload"]["content"] = json!([{"type":"output_text","text":text}]);
                    lines.remove(1);
                }
                _ => unreachable!(),
            }
            std::fs::write(
                &path,
                lines
                    .iter()
                    .map(Value::to_string)
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
            .unwrap();
            assert_eq!(
                read_codex_turn_end(&path, turn).unwrap(),
                Some(CodexTurnEnd::Final(text.into())),
                "{source}: {text:?}"
            );
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rollout_0164_requires_a_completed_final_for_the_same_turn_before_its_end() {
    let dir = rollout_dir("0164-boundaries");
    let path = dir.join("rollout-recorded.jsonl");
    let turn = "01a19a4c-21df-7ba1-93ea-4e073e8af0e0";
    for case in [
        "running",
        "foreign",
        "commentary",
        "unfinished",
        "no-text",
        "after-end",
        "user",
    ] {
        let mut lines = final_0164_records();
        match case {
            "running" => {
                lines.pop();
            }
            "foreign" => {
                lines[1]["payload"]["turn_id"] = json!("other");
                lines[2]["payload"]["internal_chat_message_metadata_passthrough"]["turn_id"] =
                    json!("other");
                lines[2]["metadata"]["retained_source"]["id"]["turn_id"] = json!("other");
            }
            "commentary" => {
                lines[1]["payload"]["item"]["phase"] = json!("commentary");
                lines[2]["payload"]["phase"] = json!("commentary");
            }
            "unfinished" => {
                lines[1]["payload"]["type"] = json!("item_started");
                lines[2]["payload"]["status"] = json!("in_progress");
            }
            "no-text" => {
                lines[1]["payload"]["item"]["content"] = json!([]);
                lines[2]["payload"]["content"] = json!([]);
            }
            "after-end" => {
                let end = lines.pop().unwrap();
                lines.insert(1, end);
            }
            "user" => {
                lines[1]["payload"]["item"]["type"] = json!("UserMessage");
                lines[2]["payload"]["role"] = json!("user");
            }
            _ => unreachable!(),
        }
        std::fs::write(
            &path,
            lines
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        let expected = if case == "running" {
            None
        } else {
            Some(CodexTurnEnd::NoReply)
        };
        assert_eq!(
            read_codex_turn_end(&path, turn).unwrap(),
            expected,
            "{case}"
        );
    }
    let mut lines = final_0164_records();
    lines[3]["payload"]["error"] = json!({"message":"failed", "codex_error_info":"provider_error"});
    std::fs::write(
        &path,
        lines
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    assert_eq!(
        read_codex_turn_end(&path, turn).unwrap(),
        Some(CodexTurnEnd::Error("failed (provider_error)".into()))
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rollout_0164_matches_the_recorded_stop_reply_without_citation_metadata() {
    let dir = rollout_dir("0164-hook-parity");
    let path = dir.join("rollout-recorded.jsonl");
    std::fs::write(&path, include_str!("fixtures/hook-captured-0.164.0.jsonl")).unwrap();
    let turn = "01a19a28-e395-7f02-a277-81cd025bb7b9";
    // The same launch's working Stop settled request 36975 with empty text.
    let hook = super::super::hooks::parse(&json!({
        "hook_event_name":"Stop", "session_id":"01a19a28-ce89-7311-8b3f-79b16616ac7f",
        "turn_id":turn, "transcript_path":path, "last_assistant_message":null
    }))
    .unwrap();
    let crate::agents::providers::spool::Parsed::Final { text, .. } = hook else {
        panic!("recorded Stop must decode as a final");
    };
    assert_eq!(text, "");
    assert_eq!(
        read_codex_turn_end(&path, turn).unwrap(),
        Some(CodexTurnEnd::Final(text))
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rollout_0164_does_not_forward_citation_only_response_metadata() {
    let dir = rollout_dir("0164-response-metadata");
    let path = dir.join("rollout-recorded.jsonl");
    let records = include_str!("fixtures/hook-captured-0.164.0.jsonl");
    let lines: Vec<_> = records
        .lines()
        .filter(|line| {
            let event: Value = serde_json::from_str(line).unwrap();
            event["payload"]["type"] != "item_completed"
        })
        .collect();
    std::fs::write(&path, lines.join("\n")).unwrap();
    assert_eq!(
        read_codex_turn_end(&path, "01a19a28-e395-7f02-a277-81cd025bb7b9").unwrap(),
        Some(CodexTurnEnd::Final("".into()))
    );
    std::fs::remove_dir_all(dir).unwrap();
}
