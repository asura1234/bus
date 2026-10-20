use super::*;
use crate::utils::time as bus_io;
use serde_json::json;
use std::path::PathBuf;

fn token_count(limits: Value) -> String {
    json!({"timestamp":"2026-10-05T00:00:00Z","type":"event_msg",
        "payload":{"type":"token_count","info":null,"rate_limits":limits}})
    .to_string()
}

#[test]
fn rollout_uses_newest_rate_limits_and_classifies_windows_by_duration() {
    let dir = std::env::temp_dir().join(format!(
        "bus-usage-{}-{}",
        std::process::id(),
        bus_io::now_ns()
    ));
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
    let dir = std::env::temp_dir().join(format!(
        "bus-turn-error-{}-{}",
        std::process::id(),
        bus_io::now_ns()
    ));
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
    assert_eq!(read_codex_turn_end(&path, "turn-2").unwrap(), None);
    assert_eq!(read_codex_turn_end(&path, "turn-9").unwrap(), None);
    assert!(read_codex_turn_end(&dir.join("missing.jsonl"), "turn-1").is_err());
    std::fs::remove_dir_all(dir).unwrap();
}

fn rollout_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "bus-{name}-{}-{}",
        std::process::id(),
        bus_io::now_ns()
    ));
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
