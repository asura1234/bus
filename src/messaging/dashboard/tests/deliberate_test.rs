use super::*;

fn report(state: &BusState) -> serde_json::Value {
    serde_json::to_value(snapshot(state, singapore_hour)).unwrap()
}

fn measured_outcomes() -> Vec<Request> {
    [
        "completed-without-text",
        "submission_unconfirmed",
        "provider_error: quota exhausted",
    ]
    .map(named_case)
    .into()
}

#[test]
fn deliberate_closures_do_not_change_dashboard_totals_rates_failures_or_latency() {
    let expected = report(&fixture_state(measured_outcomes()));
    assert_eq!(expected["all"]["rates"]["delivery"]["percent"], 50.0);
    assert_eq!(expected["failures"].as_array().unwrap().len(), 2);
    for reason in [
        "agent_deleted",
        "room_deleted",
        "interrupted",
        "background_wait_superseded",
        "github_watch_retired",
        "room_digest_retired",
        "messaging_onboarding_retired",
        "operational_notice_retired",
    ] {
        for bound in [false, true] {
            let mut omitted = named_case("agent_deleted");
            omitted.failure_reason = Some(format!(" {reason} "));
            omitted.trusted_start_bound = bound;
            let sent = omitted.prompt.submitted_at_ms;
            omitted.latency.accepted_at_ms = Some(sent);
            omitted.latency.queued_at_ms = Some(sent + 50);
            omitted.latency.picked_at_ms = Some(sent + 123);
            omitted.latency.native_requested_at_ms = Some(sent + 200);
            omitted.latency.native.native_confirmed_at_ms = Some(sent + 300);
            let id = omitted.id;
            let mut records = measured_outcomes();
            records.push(omitted);
            let state = fixture_state(records);
            let before = serde_json::to_value(&state).unwrap();
            assert_eq!(report(&state), expected, "{reason}, bound={bound}");
            assert_eq!(serde_json::to_value(&state).unwrap(), before);
            let retained = state.request(id).unwrap();
            assert_eq!(retained.delivery_stage(), "abandoned");
            assert_eq!(retained.failure_reason.as_deref().unwrap().trim(), reason);
        }
    }
}

#[test]
fn successful_outcomes_win_over_stale_deliberate_reasons() {
    for name in ["completed-without-text", "late-reply-abandoned"] {
        let mut success = named_case(name);
        success.failure_reason = None;
        let expected = report(&fixture_state([success.clone()]));
        success.failure_reason = Some("agent_deleted".into());
        let actual = report(&fixture_state([success]));
        assert_eq!(actual["all"]["rates"], expected["all"]["rates"], "{name}");
        assert_eq!(actual["all"]["counts"]["attempts"], 1);
        assert_eq!(actual["all"]["counts"]["delivered"], 1);
        assert_eq!(actual["failures"], serde_json::json!([]));
    }
}

#[test]
fn deleting_recipient_with_open_request_keeps_history_and_excludes_dashboard_attempt() {
    for bound in [false, true] {
        let mut state = fixture_state(measured_outcomes());
        let accepted = 1_793_577_600_000;
        let mut expected_state = state.clone();
        expected_state
            .delete_agent(AgentId(2), accepted + 500)
            .unwrap();
        let expected = report(&expected_state);
        let id = submit_latency(&mut state, RoomId(1), AgentId(2), accepted);
        stamp_pickup(&mut state, id, accepted, Some(accepted + 250));
        if bound {
            let mut document = serde_json::to_value(&state).unwrap();
            document["requests"][id.0.to_string()]["phase"] = "active".into();
            document["requests"][id.0.to_string()]["trusted_start_bound"] = true.into();
            state = serde_json::from_value(document).unwrap();
        }
        state.delete_agent(AgentId(2), accepted + 500).unwrap();
        let retained = state.request(id).unwrap();
        assert_eq!(retained.failure_reason.as_deref(), Some("agent_deleted"));
        assert_eq!(retained.delivery_stage(), "abandoned");
        assert_eq!(report(&state), expected, "bound={bound}");
    }
}

#[test]
fn data_route_excludes_archived_deliberate_outcomes_without_rewriting_history() {
    let root = crate::utils::test_temp::unique_temp_path("dashboard-deliberate-archive");
    crate::messaging::storage::io::private_dir(&root).unwrap();
    let state_path = root.join("state.json");
    crate::messaging::storage::state_store::JsonStore::new(state_path.clone())
        .save(&fixture_state(measured_outcomes()))
        .unwrap();
    let fetch = || {
        let response = super::super::super::response_for(&root, "GET", "/data");
        assert_eq!(response.status, "200 OK");
        serde_json::from_slice::<serde_json::Value>(&response.body).unwrap()["metrics"].clone()
    };
    let expected = fetch();
    let mut history = serde_json::to_vec(&serde_json::json!({
        "request": named_case("agent_deleted")
    }))
    .unwrap();
    history.push(b'\n');
    let history_path = root.join("history.jsonl");
    std::fs::write(&history_path, &history).unwrap();
    let before = std::fs::read(&state_path).unwrap();
    assert_eq!(fetch(), expected);
    assert_eq!(std::fs::read(&state_path).unwrap(), before);
    assert_eq!(std::fs::read(&history_path).unwrap(), history);
    std::fs::remove_dir_all(root).unwrap();
}
