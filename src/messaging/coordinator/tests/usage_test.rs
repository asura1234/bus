use super::*;
use crate::agents::providers::usage::{ObservationManifest, FIVE_HOUR_MINUTES, WEEKLY_MINUTES};

#[test]
fn claude_merge_uses_newest_observation_and_expires_without_claiming_unused_quota() {
    let now = bus_io::now_ms();
    let mut usage = Usage {
        started_at_ms: now - 1000,
        ..Usage::default()
    };
    let observation = |agent, at| Observation {
        manifest: ObservationManifest {
            agent_id: agent,
            provider: ProviderKind::ClaudeCode,
            launch_id: "launch".into(),
        },
        session_id: "session".into(),
        read_at_ms: at,
        windows: UsageWindows {
            five_hour: Some(UsageWindow {
                used_percent: 20.0,
                resets_at: Some(now / 1000 + 10),
                window_minutes: FIVE_HOUR_MINUTES,
            }),
            weekly: Some(UsageWindow {
                used_percent: 40.0,
                resets_at: Some(now / 1000 + 10000),
                window_minutes: WEEKLY_MINUTES,
            }),
        },
    };
    usage.merge_claude(observation(1, now - 2000), AgentId(1));
    assert!(
        usage.claude.is_none(),
        "old spool is not a new observation after restart"
    );
    usage.merge_claude(observation(1, now - 900), AgentId(1));
    usage.merge_claude(observation(2, now - 500), AgentId(2));
    usage.merge_claude(observation(1, now - 800), AgentId(1));
    assert_eq!(usage.claude_json(now)["observed_by_agent"], 2);
    assert_eq!(usage.claude_json(now)["five_hour"]["used_percent"], 20.0);
    assert_eq!(usage.claude_json(now + 10000)["five_hour"], Value::Null);
    assert_eq!(
        usage.claude_json(now + 10000)["weekly"]["used_percent"],
        40.0
    );
    assert_eq!(usage.claude_json(now + STALE_AFTER_MS)["status"], "unknown");
    let mut missing = observation(2, now);
    missing.windows = UsageWindows::default();
    usage.merge_claude(missing, AgentId(2));
    assert_eq!(usage.claude_json(now)["status"], "unknown");
}

#[test]
fn state_reports_missing_usage_as_unknown_never_as_unused() {
    let usage = Usage::default();
    let value = usage.state_json();
    for provider in ["claude", "codex", "cursor"] {
        assert_eq!(value[provider]["status"], "unknown", "{provider}");
        assert!(value[provider].get("five_hour").is_none(), "{provider}");
    }
    let usage = Usage {
        codex: Some(UsageSnapshot {
            windows: UsageWindows {
                five_hour: None,
                weekly: Some(UsageWindow {
                    used_percent: 7.0,
                    resets_at: Some(5),
                    window_minutes: WEEKLY_MINUTES,
                }),
            },
            read_at_ms: 9,
            observed_by_agent: AgentId(3),
        }),
        ..Usage::default()
    };
    assert_eq!(
        usage.state_json()["codex"],
        json!({"status":"observed","five_hour":null,
            "weekly":{"used_percent":7.0,"resets_at":5,"window_minutes":10080},
            "read_at_ms":9,"observed_by_agent":3})
    );
}
