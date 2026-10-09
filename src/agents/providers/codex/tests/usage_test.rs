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
