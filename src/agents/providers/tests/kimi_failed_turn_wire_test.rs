use super::*;

fn captured(name: &str) -> Vec<Value> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/kimi-failed-turn");
    serde_json::from_str(&std::fs::read_to_string(root.join(format!("{name}.json"))).unwrap())
        .unwrap()
}

fn text(rows: &[Value]) -> String {
    rows.iter().map(|row| format!("{row}\n")).collect()
}

#[test]
fn kimi_failed_turn_reordered_rr_records_close_the_owned_turn() {
    assert!(matches!(
        committed_outcome(&text(&captured("rr-owner")), 2, "Bus task"),
        Some(Outcome::Failed(_))
    ));
}

#[test]
fn kimi_failed_turn_interleaved_cc_records_close_the_owned_turn() {
    assert!(matches!(
        committed_outcome(&text(&captured("cc-owner")), 2, "Bus task"),
        Some(Outcome::Failed(_))
    ));
}

#[test]
fn kimi_failed_turn_reordered_proof_requires_the_owned_complete_chain() {
    let mut rows = captured("rr-owner");
    // Without #321's authoritative structured root error, the full agent
    // terminal proof and its native time bounds are still required.
    rows[1].as_object_mut().unwrap().remove("error");
    for removed in 0..4 {
        let mut partial = rows.clone();
        partial.remove(removed);
        assert_eq!(committed_outcome(&text(&partial), 2, "Bus task"), None);
    }
    for (index, field, value) in [
        (1, "turnId", serde_json::json!(99)),
        (2, "promptId", serde_json::json!("foreign")),
        (3, "time", serde_json::json!(1793640080958u64)),
        (3, "agentId", serde_json::json!("subagent")),
        (3, "time", Value::Null),
        (1, "reason", serde_json::json!("completed")),
    ] {
        let mut foreign = rows.clone();
        foreign[index][field] = value;
        assert_eq!(committed_outcome(&text(&foreign), 2, "Bus task"), None);
    }
    assert_eq!(committed_outcome(&text(&rows), 2, "foreign text"), None);
}

#[test]
fn kimi_failed_turn_reordered_terminal_permutations_keep_independent_agent_ids() {
    let rows = captured("rr-owner");
    for order in [
        [1, 2, 3],
        [1, 3, 2],
        [2, 1, 3],
        [2, 3, 1],
        [3, 1, 2],
        [3, 2, 1],
    ] {
        let mut reordered = vec![rows[0].clone()];
        for index in order {
            let mut row = rows[index].clone();
            if row["type"] == "agent.turn.ended" {
                row["turnId"] = serde_json::json!(99);
            }
            reordered.push(row);
        }
        assert!(matches!(
            committed_outcome(&text(&reordered), 2, "Bus task"),
            Some(Outcome::Failed(_))
        ));
    }
}

#[test]
fn kimi_failed_turn_formatter_hook_receives_only_the_owned_outer_error() {
    let rows = captured("cc-owner");
    let mut observed = rows.clone();
    let mut foreign = rows[2].clone();
    foreign["turnId"] = serde_json::json!(123);
    foreign["error"]["message"] = serde_json::json!("foreign failure");
    observed.push(foreign);
    let mut proof = failed_turn::Evidence::new(2);
    for row in &observed {
        proof.observe(row);
    }
    let committed = proof
        .committed("Bus task", |agent, outer| {
            assert_eq!(agent, Some("[object Object]"));
            outer["message"].as_str().unwrap().to_owned()
        })
        .unwrap();
    assert_eq!(
        committed.outcome,
        Outcome::Failed("429 You've reached your 5-hour usage limit.".into())
    );
    let mut proof = failed_turn::Evidence::new(2);
    for row in &observed {
        proof.observe(row);
    }
    assert!(proof
        .committed("foreign prompt", |_, _| panic!(
            "foreign input must not receive the error"
        ))
        .is_none());
}

#[test]
fn kimi_failed_turn_in_order_failure_and_success_keep_their_outcomes() {
    let mut rows = captured("rr-owner")[..4].to_vec();
    rows.sort_by_key(|row| row["time"].as_u64().unwrap());
    assert!(matches!(
        committed_outcome(&text(&rows), 2, "Bus task"),
        Some(Outcome::Failed(_))
    ));
    rows[1]["outcome"] = serde_json::json!("done");
    rows[2]["reason"] = serde_json::json!("completed");
    rows[3]["reason"] = serde_json::json!("completed");
    rows.insert(1, serde_json::json!({"type":"agent.message.appended","message":{"meta":{"source":"llm"},"message":{"role":"assistant","content":[{"type":"text","text":"success"}]}}}));
    assert_eq!(
        committed_outcome(&text(&rows), 2, "Bus task"),
        Some(Outcome::Final("success".into()))
    );
}
