use super::*;
use crate::messaging::model::{AgentId, Provider, RequestId};
use serde::Deserialize;

#[path = "deliberate_test.rs"]
mod deliberate;

#[derive(Deserialize)]
struct Case {
    name: String,
    record: Request,
    delivery: String,
    capture: String,
    cause: String,
}

fn assert_reply_capture_rate_is_absent(report: &Dashboard, label: &str) {
    let encoded = serde_json::to_value(report).unwrap();
    assert!(
        encoded["all"]["rates"].get("reply_capture").is_none(),
        "{label} still publishes reply_capture: {}",
        encoded["all"]["rates"]
    );
    assert!(
        !encoded.to_string().contains("\"reply_capture\""),
        "{label} JSON still names reply_capture"
    );
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
    let document: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/requests.json")).unwrap();
    let omitted = document["omitted_rates"]
        .as_array()
        .expect("requests fixture names dashboard rates that are gone");
    assert!(
        omitted.iter().any(|rate| rate == "reply_capture"),
        "requests fixture still re-semanticises reply capture instead of dropping it"
    );
    assert_reply_capture_rate_is_absent(&snapshot(&state, singapore_hour), "requests fixture");
}

#[test]
fn successful_ended_turns_have_no_terminal_failure_reason_in_any_series() {
    let state = fixture_state([named_case("completed-without-text")]);
    let report = snapshot(&state, singapore_hour);
    let check = |series: &Series| {
        assert_eq!(series.counts.ended, 1);
        assert!(series.counts.terminal_reasons.is_empty());
        assert!(series.hours[0].counts.terminal_reasons.is_empty());
    };
    check(&report.all);
    check(
        report
            .providers
            .values()
            .find(|s| s.counts.attempts == 1)
            .unwrap(),
    );
    check(&report.rooms.values().next().unwrap().series);
    assert!(report.failures.is_empty());

    // A genuinely abandoned request without a saved reason remains diagnosable.
    let mut abandoned = named_case("completed-without-text");
    abandoned.phase = RequestPhase::Abandoned;
    let report = snapshot(&fixture_state([abandoned]), singapore_hour);
    assert_eq!(report.all.counts.terminal_reasons["missing_reason"], 1);
    assert_eq!(report.failures[0].reason, "missing_reason");
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
        (c.replied, c.ended, c.reply_not_captured, c.reply_pending),
        (0, 1, 1, 1)
    );
    assert_eq!((c.bus_failures, c.external), (2, 1));
    assert_eq!((c.delivery_bus, c.delivery_external), (1, 1));
    assert_eq!(report.all.rates.delivery.percent, Some(75.0));
    assert_eq!(
        report.all.rates.delivery.denominator,
        c.delivered + c.delivery_bus
    );
    assert_reply_capture_rate_is_absent(&report, "hourly denominators");
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

// A conclusive 100% must name the attempts left out of that rate. Successful
// terminal deliveries are explicit replies plus turns that ended with no reply.
// Pending and external stay outside that rate.
#[test]
fn complete_bus_reliability_names_every_excluded_attempt() {
    let mut records: Vec<_> = cases()
        .into_iter()
        .take(4)
        .map(|case| case.record)
        .collect();
    records[0].phase = RequestPhase::Completed;
    records[0].failure_reason = None;
    records[1].phase = RequestPhase::Abandoned;
    records[1].failure_reason = Some("provider_error: quota".into());
    records[1].trusted_start_bound = false;
    let report = snapshot(&fixture_state(records), singapore_hour);
    let counts = &report.all.counts;
    let pending = counts.open + counts.reply_pending;
    let successes = counts.replied + counts.ended;
    assert_eq!(
        counts.attempts,
        successes + counts.bus_failures + counts.external + pending
    );
    assert!(counts.external > 0 && counts.open > 0 && counts.reply_pending > 0);
    assert_eq!(counts.bus_failures, 0);
    assert_eq!(report.all.rates.bus_reliability.percent, Some(100.0));
    assert_eq!(report.all.rates.bus_reliability.denominator, successes);
    let caption = &report.all.rates.bus_reliability.caption;
    assert!(caption.starts_with("100% of "), "{caption}");
    assert!(
        caption.contains(&format!("of {successes}/{successes} conclusive")),
        "{caption}"
    );
    assert!(caption.contains(&format!("{pending} pending")), "{caption}");
    assert!(
        caption.contains(&format!("{} external", counts.external)),
        "{caption}"
    );
    assert_ne!(caption.as_str(), "100%");
    assert!(!caption.ends_with('%'), "{caption}");
    assert_reply_capture_rate_is_absent(&report, "complete bus reliability");
    for rate in [&report.all.rates.bus_reliability] {
        if rate.percent.is_some() {
            assert!(
                rate.caption.contains("conclusive")
                    && rate
                        .caption
                        .contains(&format!("of {}/{} ", rate.numerator, rate.denominator)),
                "{}",
                rate.caption
            );
        }
    }
    assert_reliability_row(&report.all, "excluded attempts");
}

fn assert_reliability_row(series: &Series, label: &str) {
    let counts = &series.counts;
    let pending = counts.open + counts.reply_pending;
    assert_eq!(
        counts.attempts,
        counts.delivered + counts.open + counts.delivery_bus + counts.delivery_external,
        "{label}: attempted = delivered + pending + Bus failures + external"
    );
    assert_eq!(
        counts.failed,
        counts.delivery_bus + counts.delivery_external,
        "{label}"
    );
    let identity = &series.identity;
    assert!(
        identity.contains(&series.rates.delivery_share.caption),
        "{label}: {identity}"
    );
    assert!(
        identity.contains(&format!(
            "{} attempted = {} delivered + {} pending + {} Bus failures + {} external",
            counts.attempts,
            counts.delivered,
            counts.open,
            counts.delivery_bus,
            counts.delivery_external
        )),
        "{label}: {identity}"
    );
    let successes = counts.replied + counts.ended;
    let bus_denominator = successes + counts.bus_failures;
    if bus_denominator == 0 {
        assert!(!identity.contains("0/0"), "{label}: {identity}");
        assert!(
            identity.contains("Bus reliability is —"),
            "{label}: {identity}"
        );
    } else {
        assert!(
            identity.contains(&format!(
                "Of conclusive deliveries, {}",
                series.rates.bus_reliability.caption
            )),
            "{label}: {identity}"
        );
        assert!(
            !identity.contains("failed for Bus-caused reasons"),
            "{label}: sentence asks about failures beside {caption}",
            caption = series.rates.bus_reliability.caption
        );
    }
    let successes = counts.replied + counts.ended;
    assert_eq!(series.rates.bus_reliability.numerator, successes, "{label}");
    assert_eq!(
        series.rates.bus_reliability.denominator,
        successes + counts.bus_failures,
        "{label}"
    );
    let caption = &series.rates.bus_reliability.caption;
    assert!(
        caption.contains(&format!("{pending} pending")),
        "{label}: {caption}"
    );
    assert!(
        caption.contains(&format!("{} external", counts.external)),
        "{label}: {caption}"
    );
    assert_ne!(caption.as_str(), "100%", "{label}");
    if counts.replied + counts.ended + counts.bus_failures == 0 {
        assert!(caption.starts_with('—'), "{label}: {caption}");
        assert!(!caption.contains('%'), "{label}: {caption}");
    } else {
        assert!(
            caption.contains(&format!(
                "of {}/{} conclusive",
                series.rates.bus_reliability.numerator, series.rates.bus_reliability.denominator
            )),
            "{label}: {caption}"
        );
    }
}

#[test]
fn group_rows_reconcile_attempted_with_delivered_failed_external_and_pending() {
    let mut records: Vec<_> = cases()
        .into_iter()
        .take(6)
        .map(|case| case.record)
        .collect();
    records[0].phase = RequestPhase::Completed;
    records[0].failure_reason = None;
    records[0].agent_id = AgentId(2);
    records[1].phase = RequestPhase::Completed;
    records[1].failure_reason = None;
    records[1].agent_id = AgentId(3);
    records[2].phase = RequestPhase::Abandoned;
    records[2].failure_reason = Some("submission_unconfirmed".into());
    records[2].trusted_start_bound = false;
    records[2].agent_id = AgentId(3);
    records[3].phase = RequestPhase::Completed;
    records[3].failure_reason = None;
    records[3].agent_id = AgentId(4);
    records[4].phase = RequestPhase::Active;
    records[4].trusted_start_bound = true;
    records[4].failure_reason = None;
    records[4].agent_id = AgentId(4);
    records[5].phase = RequestPhase::Abandoned;
    records[5].failure_reason = Some("provider_error: quota".into());
    records[5].trusted_start_bound = false;
    records[5].agent_id = AgentId(4);
    let report = snapshot(&fixture_state(records), singapore_hour);

    let clear = &report.providers["claude"];
    assert_eq!(clear.counts.bus_failures, 0);
    assert_eq!(clear.counts.external, 0);
    assert_eq!(pending_attempts(&clear.counts), 0);
    assert_eq!(clear.rates.bus_reliability.percent, Some(100.0));
    assert_reliability_row(clear, "zero failures");

    let failed = &report.providers["codex"];
    assert!(failed.counts.bus_failures > 0, "failures");
    assert_eq!(failed.counts.replied, 0);
    assert_eq!(failed.counts.ended, 1);
    assert_eq!(failed.counts.bus_failures, 1);
    assert_reliability_row(failed, "failures");

    let waiting = &report.providers["kimi"];
    assert!(pending_attempts(&waiting.counts) > 0, "pending");
    assert!(waiting.counts.external > 0, "external beside pending");
    assert_eq!(waiting.rates.bus_reliability.percent, Some(100.0));
    assert_ne!(waiting.counts.attempts, waiting.counts.replied);
    assert_reliability_row(waiting, "pending");

    assert_reliability_row(&report.providers["cursor"], "empty provider");
    assert_reliability_row(&report.rooms["1"].series, "room");
    assert_reliability_row(&report.all, "session");
}

#[test]
fn payload_has_providers_and_rooms_and_no_recipient_breakdown() {
    let report = serde_json::to_value(snapshot(
        &fixture_state(cases().into_iter().take(1).map(|case| case.record)),
        singapore_hour,
    ))
    .unwrap();
    assert!(report.get("recipients").is_none(), "{report}");
    assert!(report.get("providers").is_some());
    assert!(report.get("rooms").is_some());
    assert!(
        report["providers"]["claude"]["counts"]["attempts"]
            .as_u64()
            .unwrap()
            >= 1
    );
    assert_eq!(report["rooms"]["1"]["name"], "dashboard");
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
    assert_reply_capture_rate_is_absent(&report, "empty denominators");
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
            .ended,
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
            .ended,
        1
    );
    assert!(state.request(RequestId(100)).is_some());
}

// Catches dropping a deleted agent's attempts from the provider total, attributing
// cross-room requests to the agent's home room, and a drill-down with no IDs.
#[test]
fn provider_room_and_failure_views_preserve_auditable_request_metadata() {
    let records = [
        named_case("completed-without-text"),
        named_case("reply-not-captured-bound"),
        named_case("provider_error: quota exhausted"),
    ];
    let mut state = fixture_state(records);
    state.delete_agent(AgentId(2), 1).unwrap();
    let report = serde_json::to_value(snapshot(&state, singapore_hour)).unwrap();
    assert!(report.get("recipients").is_none(), "{report}");
    assert_eq!(report["providers"]["claude"]["counts"]["attempts"], 3);
    assert_eq!(report["rooms"]["1"]["name"], "dashboard");
    assert_eq!(report["rooms"]["1"]["series"]["counts"]["attempts"], 3);
    let failures = report["failures"].as_array().unwrap();
    assert_eq!(failures.len(), 2);
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

#[test]
fn manual_recovery_is_counted_as_a_bus_failure_with_its_own_reason() {
    use crate::messaging::model::{Author, Draft, ManualRecovery};
    let mut state = BusState::new();
    let room = state.create_room("manual recovery").unwrap();
    let agent = state
        .create_agent(room, "kimi", Provider::Kimi, "/scratch".into(), None)
        .unwrap();
    let request = state
        .submit_message_with(
            room,
            Draft {
                text: "stuck delivery".into(),
                files: Vec::new(),
                recipient_ids: [agent].into(),
            },
            Author::Developer,
            100,
            true,
        )
        .unwrap()[0];
    state
        .manually_recover_idle_request(
            request,
            ManualRecovery {
                actor: Author::Developer,
                at_ms: 200,
            },
        )
        .unwrap();
    let report = snapshot(&state, singapore_hour);
    let counts = &report.all.counts;
    assert_eq!(
        (
            counts.attempts,
            counts.failed,
            counts.bus_failures,
            counts.external
        ),
        (1, 1, 1, 0)
    );
    assert_eq!(counts.bus_reasons["manual_recovery"], 1);
    assert_eq!(counts.external_reasons.get("manual_recovery"), None);
    assert_eq!(report.all.rates.bus_reliability.numerator, 0);
    assert_eq!(report.all.rates.bus_reliability.denominator, 1);
    assert_eq!(report.all.rates.bus_reliability.percent, Some(0.0));
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].reason, "manual_recovery");
    assert_eq!(report.failures[0].cause, "bus");
}

#[test]
fn dashboard_provider_hold_and_retry_remain_pending_without_failure_counters() {
    use crate::messaging::model::{
        AgentRuntimeIdentity, CallbackDisposition, CallbackEventKind, ProviderCallback,
        RuntimeStatus, SubmissionOutcome,
    };
    let mut state = BusState::new();
    let room = state.create_room("retry-dashboard").unwrap();
    let agent = state
        .create_agent(room, "codex", Provider::Codex, "/scratch".into(), None)
        .unwrap();
    state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                session_id: Some("session".into()),
                ..Default::default()
            },
        )
        .unwrap();
    state.confirm_hook_setup(agent).unwrap();
    state
        .observe_status(agent, RuntimeStatus::Idle, 10)
        .unwrap();
    state.set_draft_text(room, "retain this delivery").unwrap();
    state.set_draft_recipients(room, [agent]).unwrap();
    let request = state.submit_draft(room, 10).unwrap()[0];
    state.begin_submission(request, "launch", 10).unwrap();
    state
        .record_submission(
            request,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("session".into()),
                provider_turn_id: Some("turn-1".into()),
            },
        )
        .unwrap();
    let started = ProviderCallback {
        callback_id: "dashboard-start".into(),
        sequence: 11,
        occurred_at_ms: 11,
        agent_id: agent,
        launch_id: "launch".into(),
        provider_session_id: Some("session".into()),
        provider_turn_id: Some("turn-1".into()),
        provider_prompt_id: None,
        prompt_payload: Some(state.request(request).unwrap().prompt.rendered_payload()),
        kind: CallbackEventKind::PromptStarted,
    };
    assert_eq!(
        state.accept_callback(started.clone()),
        CallbackDisposition::AcceptedBinding
    );
    assert_eq!(
        state.accept_callback(ProviderCallback {
            callback_id: "dashboard-overload".into(),
            sequence: 12,
            occurred_at_ms: 100,
            prompt_payload: None,
            kind: CallbackEventKind::Error {
                message: "Provider failed".into(),
                provider_error: Some("Selected model is at capacity (server_overloaded)".into())
            },
            ..started.clone()
        }),
        CallbackDisposition::AcceptedError
    );
    let due = state
        .agent(agent)
        .unwrap()
        .provider_retry
        .as_ref()
        .unwrap()
        .retry_at_ms;
    for (phase, delivery, stage) in [
        (0, "delivered", "delivered"),
        (1, "open", "queued"),
        (2, "open", "submitting"),
        (3, "delivered", "delivered"),
    ] {
        match phase {
            1 => {
                assert!(state.retry_provider_requests(due));
            }
            2 => {
                state.begin_submission(request, "launch", 12).unwrap();
            }
            3 => {
                state
                    .record_submission(
                        request,
                        SubmissionOutcome::Confirmed {
                            provider_session_id: Some("session".into()),
                            provider_turn_id: Some("turn-2".into()),
                        },
                    )
                    .unwrap();
                let payload = state.request(request).unwrap().submitted_payload.clone();
                assert_eq!(
                    state.accept_callback(ProviderCallback {
                        callback_id: "dashboard-retry-start".into(),
                        sequence: 20,
                        occurred_at_ms: due + 1,
                        provider_turn_id: Some("turn-2".into()),
                        prompt_payload: payload,
                        ..started.clone()
                    }),
                    CallbackDisposition::AcceptedBinding
                );
            }
            _ => {}
        }
        let record = state.request(request).unwrap();
        assert!(!record.settled(), "phase {phase}");
        assert!(record.failure_reason.is_none());
        assert_eq!(record.delivery_stage(), stage);
        let outcome = classify(&state, record);
        assert_eq!(
            (outcome.delivery, outcome.capture, outcome.cause),
            (delivery, "pending", "pending")
        );
        let report = snapshot(&state, singapore_hour);
        let c = &report.all.counts;
        assert_eq!(c.attempts, 1, "one original request across retries");
        assert_eq!(
            (c.failed, c.reply_not_captured, c.bus_failures, c.external),
            (0, 0, 0, 0)
        );
        assert_eq!(
            (c.delivered, c.open, c.reply_pending),
            if delivery == "delivered" {
                (1, 0, 1)
            } else {
                (0, 1, 0)
            }
        );
        assert!(report.failures.is_empty());
        assert_reply_capture_rate_is_absent(&report, "retry stays pending");
        assert_eq!(report.all.rates.bus_reliability.denominator, 0);
        if phase <= 1 {
            assert_eq!(
                state.agent(agent).unwrap().shown_status(),
                RuntimeStatus::Working
            );
        }
    }
}

fn named_case(name: &str) -> super::Request {
    cases()
        .into_iter()
        .find(|case| case.name == name)
        .unwrap_or_else(|| panic!("missing fixture {name}"))
        .record
}

fn assert_absent_bus_rate(series: &super::Series, label: &str) {
    assert_eq!(
        series.rates.bus_reliability.denominator, 0,
        "{label} has no conclusive deliveries"
    );
    assert!(
        series.rates.bus_reliability.caption.starts_with('—'),
        "{label} card: {}",
        series.rates.bus_reliability.caption
    );
    assert!(
        !series.identity.contains("0/0"),
        "{label} counts line still prints 0/0: {}",
        series.identity
    );
    assert!(
        series.identity.contains('—'),
        "{label} counts line omits the card's em dash: {}",
        series.identity
    );
}

// An empty session, an all-pending session, and an all-external session have
// no conclusive delivery rate. The counts line must say so with the card's em dash.
#[test]
fn dashboard_wording_absent_rate_for_empty_all_pending_and_all_external() {
    let empty = snapshot(&fixture_state(std::iter::empty()), singapore_hour);
    assert_eq!(empty.all.counts.attempts, 0);
    assert_eq!(empty.all.rates.delivery_share.caption, "—");
    assert_eq!(empty.all.counts.oldest_open_ms, None);
    assert_absent_bus_rate(&empty.all, "empty");

    let open = named_case("awaiting-start");
    let sent = open.prompt.submitted_at_ms;
    let pending = snapshot(&fixture_state([open]), singapore_hour);
    assert!(pending.all.counts.open > 0, "all pending");
    assert_eq!(pending.all.counts.replied, 0);
    assert_eq!(pending.all.counts.bus_failures, 0);
    assert_eq!(pending.all.counts.oldest_open_ms, Some(sent));
    assert_absent_bus_rate(&pending.all, "all pending");

    let external = snapshot(
        &fixture_state([named_case("provider_error: quota exhausted")]),
        singapore_hour,
    );
    assert!(external.all.counts.external > 0, "all external");
    assert_eq!(external.all.counts.replied, 0);
    assert_eq!(external.all.counts.bus_failures, 0);
    assert_eq!(external.all.counts.delivered, 0);
    assert_eq!(external.all.counts.oldest_open_ms, None);
    assert_absent_bus_rate(&external.all, "all external");
}

// One completed delivery plus one bound reply_not_captured must not call the
// reply miss a delivery failure. The card stays 2/2 delivered with 0 Bus failures.
#[test]
fn dashboard_wording_identity_matches_delivery_counters() {
    let report = snapshot(
        &fixture_state([
            named_case("completed-without-text"),
            named_case("reply-not-captured-bound"),
        ]),
        singapore_hour,
    );
    let counts = &report.all.counts;
    assert_eq!(counts.delivered, 2);
    assert_eq!(counts.failed, 0);
    assert_eq!(counts.replied, 0);
    assert_eq!(counts.ended, 1);
    assert_eq!(counts.reply_not_captured, 1);
    assert_eq!(counts.bus_failures, 1);
    let card = &report.all.rates.delivery_share.caption;
    assert!(card.contains("2/2"), "{card}");
    assert!(card.contains("0 Bus failures"), "{card}");
    let identity = &report.all.identity;
    assert!(
        identity.contains(card),
        "sentence and delivery card disagree: {identity} vs {card}"
    );
    assert!(
        identity.contains("2 delivered"),
        "sentence and delivery card disagree: {identity} vs {card}"
    );
    assert!(
        identity.contains("0 Bus failures"),
        "sentence uses a different failed counter than the card ({card}): {identity}"
    );
    assert!(!identity.contains("1 delivered + 1 failed"), "{identity}");
}

// Zero Bus failures is a 100% success caption. The counts line must repeat
// that caption, not ask how many failed.
#[test]
fn dashboard_wording_zero_bus_failures_match_the_success_caption() {
    let report = snapshot(
        &fixture_state([named_case("completed-without-text")]),
        singapore_hour,
    );
    let counts = &report.all.counts;
    assert_eq!(counts.bus_failures, 0);
    assert_eq!(counts.replied, 0);
    assert!(
        counts.ended > 0,
        "a completed turn is a conclusive delivery"
    );
    let caption = &report.all.rates.bus_reliability.caption;
    assert_eq!(report.all.rates.bus_reliability.percent, Some(100.0));
    assert!(caption.starts_with("100%"), "{caption}");
    assert!(
        caption.contains(&format!("of {}/{} conclusive", counts.ended, counts.ended)),
        "{caption}"
    );
    let identity = &report.all.identity;
    assert!(
        identity.contains(&format!("Of conclusive deliveries, {caption}")),
        "{identity}"
    );
    assert!(!identity.contains("how many failed"), "{identity}");
    assert!(
        !identity.contains("failed for Bus-caused reasons"),
        "{identity}"
    );
}

// A message Bus delivered, with no reply yet, is a delivery success.
#[test]
fn dashboard_wording_unanswered_delivery_is_success() {
    let report = snapshot(&fixture_state([named_case("delivered")]), singapore_hour);
    let counts = &report.all.counts;
    assert_eq!(counts.attempts, 1);
    assert_eq!(counts.delivered, 1);
    assert_eq!(counts.failed, 0);
    assert_eq!(counts.replied, 0);
    assert_eq!(counts.reply_pending, 1);
    assert_eq!(counts.delivery_bus, 0);
    assert_eq!(counts.delivery_external, 0);
    assert_eq!(report.all.rates.delivery_share.percent, Some(100.0));
    assert!(
        report.all.identity.contains("1 delivered"),
        "{}",
        report.all.identity
    );
    assert!(
        report
            .all
            .identity
            .contains(&report.all.rates.delivery_share.caption),
        "{}",
        report.all.identity
    );
    assert!(
        !report.all.identity.contains("1 failed"),
        "{}",
        report.all.identity
    );
    assert_absent_bus_rate(&report.all, "delivered without a reply");
}

// Delivery is delivered / (delivered + Bus-caused delivery failures).
// An external failure stays on the card and out of the denominator.
// Pending stays out of the denominator too.
#[test]
fn delivery_rate_excludes_external_failures_and_pending() {
    let external_only = snapshot(
        &fixture_state([
            named_case("completed-without-text"),
            named_case("provider_error: quota exhausted"),
        ]),
        singapore_hour,
    );
    let counts = &external_only.all.counts;
    assert_eq!(counts.delivered, 1);
    assert_eq!(counts.delivery_bus, 0);
    assert_eq!(counts.delivery_external, 1);
    assert_eq!(counts.open, 0);
    assert_eq!(counts.failed, 1);
    for rate in [
        &external_only.all.rates.delivery,
        &external_only.all.rates.delivery_share,
    ] {
        assert_eq!(rate.percent, Some(100.0), "{}", rate.caption);
        assert_eq!(rate.numerator, 1, "{}", rate.caption);
        assert_eq!(rate.denominator, 1, "{}", rate.caption);
        assert!(rate.caption.contains("1/1 delivered"), "{}", rate.caption);
        assert!(rate.caption.contains("0 Bus failures"), "{}", rate.caption);
        assert!(
            rate.caption.contains("1 external failure"),
            "{}",
            rate.caption
        );
        assert!(
            !rate.caption.contains("1/2"),
            "external failure entered the fraction: {}",
            rate.caption
        );
    }
    assert!(
        external_only
            .all
            .identity
            .contains(&external_only.all.rates.delivery_share.caption),
        "{}",
        external_only.all.identity
    );

    let bus_failure = snapshot(
        &fixture_state([
            named_case("completed-without-text"),
            named_case("submission_unconfirmed"),
        ]),
        singapore_hour,
    );
    let counts = &bus_failure.all.counts;
    assert_eq!(counts.delivered, 1);
    assert_eq!(counts.delivery_bus, 1);
    assert_eq!(counts.delivery_external, 0);
    assert_eq!(counts.open, 0);
    for rate in [
        &bus_failure.all.rates.delivery,
        &bus_failure.all.rates.delivery_share,
    ] {
        assert_eq!(rate.percent, Some(50.0), "{}", rate.caption);
        assert_eq!(rate.numerator, 1, "{}", rate.caption);
        assert_eq!(
            rate.denominator,
            counts.delivered + counts.delivery_bus,
            "{}",
            rate.caption
        );
        assert_eq!(rate.denominator, 2, "{}", rate.caption);
        assert!(rate.caption.contains("1/2 delivered"), "{}", rate.caption);
        assert!(rate.caption.contains("1 Bus failure"), "{}", rate.caption);
        assert!(
            rate.caption.contains("0 external failures"),
            "{}",
            rate.caption
        );
    }

    let pending = snapshot(
        &fixture_state([
            named_case("completed-without-text"),
            named_case("awaiting-start"),
        ]),
        singapore_hour,
    );
    let counts = &pending.all.counts;
    assert_eq!(counts.delivered, 1);
    assert!(counts.open >= 1, "pending stays visible");
    assert_eq!(counts.delivery_bus, 0);
    assert_eq!(counts.failed, 0);
    for rate in [
        &pending.all.rates.delivery,
        &pending.all.rates.delivery_share,
    ] {
        assert_eq!(rate.percent, Some(100.0), "{}", rate.caption);
        assert_eq!(rate.denominator, 1, "{}", rate.caption);
        assert_ne!(
            rate.denominator, counts.attempts,
            "pending entered the denominator: {}",
            rate.caption
        );
        assert!(rate.caption.contains("1/1 delivered"), "{}", rate.caption);
        assert!(rate.caption.contains("0 Bus failures"), "{}", rate.caption);
    }
}

// External-only and pending-only hours have no Bus denominator, so Delivery %
// is absent. Drawing that absence as 0% puts the excluded outcome back on the axis.
#[test]
fn absent_delivery_rate_is_not_drawn_as_zero_percent() {
    let external_only = snapshot(
        &fixture_state([named_case("provider_error: quota exhausted")]),
        singapore_hour,
    );
    let hour = &external_only.all.hours[0];
    assert!(hour.counts.attempts >= 1);
    assert!(hour.counts.delivery_external >= 1);
    assert_eq!(hour.counts.delivered, 0);
    assert_eq!(hour.counts.delivery_bus, 0);
    assert_eq!(hour.rates.delivery.percent, None);
    assert_eq!(hour.rates.delivery_share.percent, None);
    assert_eq!(hour.rates.delivery_share.denominator, 0);
    assert!(
        !hour.rates.delivery_share.caption.contains("0%"),
        "{}",
        hour.rates.delivery_share.caption
    );
    assert!(
        hour.rates
            .delivery_share
            .caption
            .contains("1 external failure"),
        "{}",
        hour.rates.delivery_share.caption
    );

    let pending = snapshot(
        &fixture_state([named_case("awaiting-start")]),
        singapore_hour,
    );
    let pending_hour = &pending.all.hours[0];
    assert!(pending_hour.counts.open >= 1);
    assert_eq!(pending_hour.counts.delivery_bus, 0);
    assert_eq!(pending_hour.counts.delivered, 0);
    assert_eq!(pending_hour.rates.delivery_share.percent, None);
    assert_eq!(pending_hour.rates.delivery_share.denominator, 0);

    let page = include_str!("../page.html");
    let graph = page
        .split_once("function renderGraph")
        .expect("renderGraph")
        .1
        .split_once("function renderDetail")
        .expect("renderDetail")
        .0;
    assert!(
        !graph.contains("delivery_share.percent??0"),
        "absent Delivery % is plotted as 0, so an external-only or pending-only hour sits on the axis"
    );
}

fn submit_latency(
    state: &mut BusState,
    room: crate::messaging::model::RoomId,
    agent: AgentId,
    accepted: u64,
) -> RequestId {
    state.set_draft_recipients(room, [agent]).unwrap();
    let text = format!("latency {accepted}");
    state.set_draft_text(room, &text).unwrap();
    state.submit_draft(room, accepted).unwrap()[0]
}

fn stamp_pickup(state: &mut BusState, id: RequestId, accepted: u64, picked: Option<u64>) {
    let mut document = serde_json::to_value(&*state).unwrap();
    document["requests"][id.0.to_string()]["latency"] = serde_json::json!({
        "accepted_at_ms": accepted,
        "picked_at_ms": picked,
    });
    *state = serde_json::from_value(document).unwrap();
}

#[test]
fn delivery_table_accepted_to_picked_keeps_provider_and_room_percentiles() {
    let mut state = BusState::new();
    let alpha = state.create_room("alpha").unwrap();
    let beta = state.create_room("beta").unwrap();
    let gamma = state.create_room("gamma").unwrap();
    let claude = state
        .create_agent(
            alpha,
            "claude",
            Provider::ClaudeCode,
            "/scratch".into(),
            None,
        )
        .unwrap();
    let codex = state
        .create_agent(beta, "codex", Provider::Codex, "/scratch".into(), None)
        .unwrap();
    let kimi = state
        .create_agent(gamma, "kimi", Provider::Kimi, "/scratch".into(), None)
        .unwrap();
    let base = 1_793_577_600_000;
    let fast = submit_latency(&mut state, alpha, claude, base);
    let slow = submit_latency(&mut state, alpha, claude, base + 1_000);
    let middle = submit_latency(&mut state, beta, codex, base + 2_000);
    let reversed = submit_latency(&mut state, gamma, kimi, base + 3_000);
    let incomplete = submit_latency(&mut state, alpha, claude, base + 4_000);
    stamp_pickup(&mut state, fast, base, Some(base + 100));
    stamp_pickup(&mut state, slow, base + 1_000, Some(base + 1_300));
    stamp_pickup(&mut state, middle, base + 2_000, Some(base + 2_200));
    stamp_pickup(&mut state, reversed, base + 3_000, Some(base + 2_950));
    stamp_pickup(&mut state, incomplete, base + 4_000, None);

    let report = snapshot(&state, singapore_hour);
    let picked = &report.all.accepted_to_picked;
    assert_eq!(
        (picked.n, picked.p50, picked.p90, picked.p99, picked.mean),
        (3, Some(200), Some(300), Some(300), Some(200))
    );
    let claude_rate = &report.providers["claude"].accepted_to_picked;
    assert_eq!(
        (
            claude_rate.n,
            claude_rate.p50,
            claude_rate.p90,
            claude_rate.p99,
            claude_rate.mean
        ),
        (2, Some(100), Some(300), Some(300), Some(200))
    );
    let codex_rate = &report.providers["codex"].accepted_to_picked;
    assert_eq!(
        (
            codex_rate.n,
            codex_rate.p50,
            codex_rate.p99,
            codex_rate.mean
        ),
        (1, Some(200), Some(200), Some(200))
    );
    for provider in ["kimi", "grok", "cursor"] {
        let empty = &report.providers[provider].accepted_to_picked;
        assert_eq!(empty.n, 0, "{provider}");
        assert_eq!(empty.p50, None, "{provider}");
        assert_eq!(empty.mean, None, "{provider}");
    }
    let alpha_rate = &report.rooms[&alpha.0.to_string()].series.accepted_to_picked;
    assert_eq!(
        (alpha_rate.n, alpha_rate.p50, alpha_rate.p99),
        (2, Some(100), Some(300))
    );
    let beta_rate = &report.rooms[&beta.0.to_string()].series.accepted_to_picked;
    assert_eq!((beta_rate.n, beta_rate.p50), (1, Some(200)));
    let gamma_rate = &report.rooms[&gamma.0.to_string()].series.accepted_to_picked;
    assert_eq!(gamma_rate.n, 0);
    assert_eq!(gamma_rate.p50, None);
    assert_eq!(gamma_rate.mean, None);
    let encoded = serde_json::to_value(&report).unwrap();
    assert!(encoded["all"].get("end_to_end").is_none());
    assert!(encoded["providers"]["claude"].get("end_to_end").is_none());
    assert!(encoded["rooms"][&alpha.0.to_string()]["series"]
        .get("end_to_end")
        .is_none());
    let raw = encoded.to_string();
    assert!(!raw.contains("reply_delivered_at"));
    assert!(!raw.contains("reply_captured_at"));
    assert!(!raw.contains("end_to_end"));
    assert_eq!(report.providers["kimi"].accepted_to_picked.n, 0);
}
