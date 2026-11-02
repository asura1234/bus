use super::{committed_outcome, main_turn, MainTurn, Outcome};
use serde_json::Value;

const OAUTH: &str = include_str!("fixtures/kimi-oauth-error.jsonl");
const QUOTA: &str = include_str!("fixtures/kimi-quota-error.jsonl");
const OAUTH_ERROR: &str = "OAuthConnectionError: OAuth request to https://auth.kimi.com/api/oauth/token failed: fetch failed: connect EHOSTUNREACH 192.0.2.10:443 - Local (192.0.2.20:49970)";
const QUOTA_ERROR: &str = "APIProviderQuotaExhaustedError: 429 You've reached your 5-hour usage limit. Your quota will reset when the current 5-hour window ends. To continue now, purchase extra usage or upgrade your plan: https://www.kimi.com/membership/subscription?tab=quota";

fn records(wire: &str) -> Vec<Value> {
    wire.lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn encode(rows: &[Value]) -> String {
    rows.iter().map(|row| format!("{row}\n")).collect()
}

#[test]
fn kimi_error_end_preserves_root_error_when_inner_failure_arrives_after_completion() {
    for (wire, turn, error) in [(OAUTH, 1, OAUTH_ERROR), (QUOTA, 0, QUOTA_ERROR)] {
        assert_eq!(
            committed_outcome(wire, turn, "Bus task"),
            Some(Outcome::Failed(error.into()))
        );
        assert_eq!(main_turn(wire).unwrap(), Some(MainTurn::Settled));
    }
}

#[test]
fn kimi_error_end_commits_the_owned_root_failure_without_waiting_for_inner_flush() {
    for (wire, turn, error) in [(OAUTH, 1, OAUTH_ERROR), (QUOTA, 0, QUOTA_ERROR)] {
        let mut rows = records(wire);
        rows.retain(|row| row["type"] != "agent.turn.ended");
        let committed = encode(&rows);
        assert_eq!(
            committed_outcome(&committed, turn, "Bus task"),
            Some(Outcome::Failed(error.into()))
        );
        assert_eq!(main_turn(&committed).unwrap(), Some(MainTurn::Settled));
        rows.pop();
        assert_eq!(committed_outcome(&encode(&rows), turn, "Bus task"), None);
        assert_eq!(main_turn(&encode(&rows)).unwrap(), Some(MainTurn::Open));
    }
}

#[test]
fn kimi_error_end_prefers_structured_quota_details_to_the_object_string() {
    let mut rows = records(QUOTA);
    let inner = rows.pop().unwrap();
    rows.insert(1, inner);
    assert_eq!(
        committed_outcome(&encode(&rows), 0, "Bus task"),
        Some(Outcome::Failed(QUOTA_ERROR.into()))
    );
}

#[test]
fn kimi_error_end_never_commits_foreign_or_incomplete_terminal_evidence() {
    for case in 0..5 {
        let mut rows = records(OAUTH);
        match case {
            0 => {
                rows.remove(1);
            }
            1 => {
                rows.remove(2);
            }
            2 => {
                rows[1]["agentId"] = "child".into();
            }
            3 => {
                rows[2]["promptId"] = "foreign".into();
            }
            _ => {
                rows[1]["turnId"] = 9.into();
            }
        }
        assert_eq!(
            committed_outcome(&encode(&rows), 1, "Bus task"),
            None,
            "case {case}"
        );
    }
    assert_eq!(committed_outcome(OAUTH, 1, "another task"), None);
    assert_eq!(committed_outcome(OAUTH, 9, "Bus task"), None);
}
