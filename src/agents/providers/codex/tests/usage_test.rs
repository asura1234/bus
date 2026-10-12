use super::*;
use crate::utils::time as bus_io;
use serde_json::json;

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
        read_codex_turn_error(&path, "turn-1").unwrap().as_deref(),
        Some("Selected model is at capacity. Please try a different model. (server_overloaded)")
    );
    assert_eq!(read_codex_turn_error(&path, "turn-2").unwrap(), None);
    assert_eq!(read_codex_turn_error(&path, "turn-9").unwrap(), None);
    assert!(read_codex_turn_error(&dir.join("missing.jsonl"), "turn-1").is_err());
    std::fs::remove_dir_all(dir).unwrap();
}
