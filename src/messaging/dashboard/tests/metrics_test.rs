use super::*;
use crate::messaging::model::{AgentId, Provider, RequestId};
use serde::Deserialize;

#[derive(Deserialize)]
struct Case {
    name: String,
    record: Request,
    delivery: String,
    capture: String,
    cause: String,
}

fn cases() -> Vec<Case> {
    {
        let mut cases: Vec<Case> =
            serde_json::from_value(serde_json::Value::Array(super::super::fixtures::cases()))
                .unwrap();
        cases.extend(
            serde_json::from_str::<Vec<Case>>(include_str!("fixtures/recorded.json")).unwrap(),
        );
        cases
    }
}

fn fixture_state(records: impl IntoIterator<Item = Request>) -> BusState {
    let mut state = BusState::new();
    let room = state.create_room("dashboard").unwrap();
    for (name, provider) in [
        ("claude", Provider::ClaudeCode),
        ("codex", Provider::Codex),
        ("kimi", Provider::Kimi),
        ("grok", Provider::Grok),
        ("cursor", Provider::Cursor),
    ] {
        state
            .create_agent(room, name, provider, "/scratch".into(), None)
            .unwrap();
    }
    let mut document = serde_json::to_value(state).unwrap();
    for record in records {
        document["requests"][record.id.0.to_string()] = serde_json::to_value(&record).unwrap();
    }
    serde_json::from_value(document).unwrap()
}

// Catches conflating a failed reply capture with a failed terminal submission,
// and counting deliberate interrupts/provider errors as Bus-caused failures.
#[test]
fn recorded_outcomes_classify_every_phase_and_preserve_reply_precedence() {
    let cases = cases();
    let state = fixture_state(cases.iter().map(|case| case.record.clone()));
    for case in cases {
        let actual = classify(&state, &case.record);
        assert_eq!(actual.delivery, case.delivery, "{} delivery", case.name);
        assert_eq!(actual.capture, case.capture, "{} capture", case.name);
        assert_eq!(actual.cause, case.cause, "{} cause", case.name);
    }
}

// Each recipient is counted even when two requests share the same prompt.
#[test]
fn hours_count_requests_at_send_time_and_keep_provider_and_reason_denominators() {
    let mut records: Vec<_> = cases().into_iter().take(5).map(|c| c.record).collect();
    records[0].phase = RequestPhase::Completed;
    records[1].phase = RequestPhase::Abandoned;
    records[1].failure_reason = Some("submission_unconfirmed".into());
    records[2].phase = RequestPhase::Abandoned;
    records[2].failure_reason = Some("provider_error: quota".into());
    records[2].agent_id = AgentId(3);
    records[3].prompt.submitted_at_ms = 1793577600000;
    records[3].agent_id = AgentId(4);
    records[4].prompt.id = records[0].prompt.id;
    records[4].agent_id = AgentId(5);
    let state = fixture_state(records);
    let report = snapshot(&state, singapore_hour);
    let c = &report.all.counts;
    assert_eq!((c.attempts, c.delivered, c.failed, c.open), (5, 3, 2, 0));
    assert_eq!(
        (c.replied, c.reply_not_captured, c.reply_pending),
        (1, 1, 1)
    );
    assert_eq!((c.bus_failures, c.external), (2, 1));
    assert_eq!(report.all.rates.delivery.percent, Some(60.0));
    assert_eq!(report.all.rates.reply_capture.percent, Some(50.0));
    assert!((report.all.rates.bus_reliability.percent.unwrap() - 100.0 / 3.0).abs() < 0.001);
    assert_eq!(c.failure_reasons["submission_unconfirmed"], 1);
    assert_eq!(c.bus_reasons["reply_not_captured"], 1);
    assert_eq!(c.external_reasons["provider_error: quota"], 1);
    assert_eq!(report.all.hours.len(), 2);
    assert_eq!(report.all.hours[0].hour, "2026-11-02 07:00 UTC+08:00");
    assert_eq!(report.all.hours[0].counts.attempts, 4);
    assert_eq!(report.all.hours[1].hour, "2026-11-02 08:00 UTC+08:00");
    assert_eq!(report.providers["claude"].counts.attempts, 2);
    assert_eq!(report.providers["codex"].counts.external, 1);
    assert_eq!(report.providers["kimi"].counts.reply_pending, 1);
    assert_eq!(report.providers["grok"].counts.reply_not_captured, 1);
    assert_eq!(report.providers["cursor"].counts.attempts, 0);
}

fn singapore_hour(ms: u64) -> HourKey {
    hour_with_offset(ms, 8 * 3600).unwrap()
}

#[test]
fn local_hour_boundary_and_repeated_dst_hours_stay_distinct() {
    assert_eq!(
        singapore_hour(1793577599999).label,
        "2026-11-02 07:00 UTC+08:00"
    );
    assert_eq!(
        singapore_hour(1793577600000).label,
        "2026-11-02 08:00 UTC+08:00"
    );
    let early = hour_with_offset(1793511000000, -4 * 3600).unwrap();
    let late = hour_with_offset(1793514600000, -5 * 3600).unwrap();
    assert_eq!(early.label, "2026-11-01 01:00 UTC-04:00");
    assert_eq!(late.label, "2026-11-01 01:00 UTC-05:00");
    assert_ne!(early.start_ms, late.start_ms);
    assert!(hour_with_offset(u64::MAX, 0).is_none());
}

#[test]
fn empty_denominators_are_absent_and_missing_reasons_are_bus_caused() {
    let report = snapshot(&BusState::new(), singapore_hour);
    assert_eq!(report.all.rates.delivery.percent, None);
    assert_eq!(report.all.rates.reply_capture.percent, None);
    assert_eq!(report.all.rates.bus_reliability.percent, None);
    assert!(report.all.hours.is_empty());
    let state = fixture_state(
        cases()
            .into_iter()
            .filter(|c| c.name == "missing-reason")
            .map(|c| c.record),
    );
    let report = snapshot(&state, singapore_hour);
    assert_eq!(report.all.counts.bus_reasons["missing_reason"], 1);
}

#[test]
fn retired_provider_identity_is_used_and_legacy_unknown_is_not_guessed() {
    let mut record = cases().remove(0).record;
    record.phase = RequestPhase::Completed;
    let mut state = fixture_state([record]);
    state.delete_agent(AgentId(2), 1).unwrap();
    assert_eq!(
        snapshot(&state, singapore_hour).providers["claude"]
            .counts
            .replied,
        1
    );
    let mut document = serde_json::to_value(state).unwrap();
    document["retired_agents"]["2"]
        .as_object_mut()
        .unwrap()
        .remove("provider");
    let state = serde_json::from_value(document).unwrap();
    assert_eq!(
        snapshot(&state, singapore_hour).providers["unknown"]
            .counts
            .replied,
        1
    );
    assert!(state.request(RequestId(100)).is_some());
}

// Catches aggregating only providers, losing deleted recipients, attributing
// cross-room requests to the agent's home room, and a drill-down with no IDs.
#[test]
fn recipient_room_and_failure_views_preserve_auditable_request_metadata() {
    let cases = cases();
    let records = [
        cases[0].record.clone(),
        cases[4].record.clone(),
        cases[15].record.clone(),
    ];
    let mut state = fixture_state(records);
    state.delete_agent(AgentId(2), 1).unwrap();
    let report = serde_json::to_value(snapshot(&state, singapore_hour)).unwrap();
    assert_eq!(report["recipients"]["2"]["name"], "claude [deleted]");
    assert_eq!(report["recipients"]["2"]["series"]["counts"]["attempts"], 3);
    assert_eq!(report["rooms"]["1"]["name"], "dashboard");
    assert_eq!(report["rooms"]["1"]["series"]["counts"]["attempts"], 3);
    let failures = report["failures"].as_array().unwrap();
    assert_eq!(failures.len(), 3);
    let lost = failures.iter().find(|f| f["request_id"] == 104).unwrap();
    assert_eq!(lost["message_id"], 94);
    assert_eq!(lost["recipient_id"], 2);
    assert_eq!(lost["provider"], "claude");
    assert_eq!(lost["room_id"], 1);
    assert_eq!(lost["sent_at_ms"], 1793577599999_u64);
    assert_eq!(lost["hour"], "2026-11-02 07:00 UTC+08:00");
    assert_eq!(lost["stage"], "abandoned");
    assert_eq!(lost["reason"], "reply_not_captured");
    assert_eq!(lost["cause"], "bus");
    assert_eq!(lost["delivery"], "delivered");
}

#[test]
fn abandoned_steering_never_borrows_a_leads_binding_before_its_own_paste() {
    use crate::messaging::model::SubmissionOutcome;
    for confirmed in [false, true] {
        let cases = cases();
        let state = fixture_state([cases[0].record.clone(), cases[3].record.clone()]);
        let mut doc = serde_json::to_value(state).unwrap();
        doc["agents"]["2"]["status"] = serde_json::json!("working");
        doc["agents"]["2"]["current_request"] = serde_json::json!(103);
        doc["queues"]["2"] = serde_json::json!([100]);
        let mut state: BusState = serde_json::from_value(doc).unwrap();
        state
            .begin_steering(RequestId(100), RequestId(103))
            .unwrap();
        if confirmed {
            state
                .record_steering(
                    RequestId(100),
                    SubmissionOutcome::Confirmed {
                        provider_session_id: None,
                        provider_turn_id: None,
                    },
                )
                .unwrap();
        }
        assert_eq!(
            classify(&state, state.request(RequestId(100)).unwrap()).delivery,
            if confirmed { "delivered" } else { "open" }
        );
        state.delete_agent(AgentId(2), 1793581201000).unwrap();
        // Abandonment erases the Active/Submitting distinction. Without a
        // request-specific start receipt, legacy steered records are unconfirmed.
        assert_eq!(
            classify(&state, state.request(RequestId(100)).unwrap()).delivery,
            "failed"
        );
    }
}
