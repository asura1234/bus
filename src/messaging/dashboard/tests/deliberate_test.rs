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

const NOTICE_AT: u64 = 1_793_577_600_000;

fn finish_worker_request(state: &mut BusState, room: RoomId, agent: AgentId) {
    let at = NOTICE_AT - 122_000;
    let request = submit_latency(state, room, agent, at);
    state.finish_for_test(request, "done", at);
}

fn idle_notice_state() -> (BusState, RoomId, AgentId, AgentId) {
    use crate::messaging::model::{AgentRuntimeIdentity, RuntimeStatus};

    let mut state = BusState::new();
    let master = state.ensure_master_room();
    let room = state.create_room("idle-notice").unwrap();
    let subject = state
        .create_agent(
            room,
            "idle-worker",
            Provider::ClaudeCode,
            "/scratch".into(),
            None,
        )
        .unwrap();
    let lead = state
        .create_agent(
            master,
            "lead",
            Provider::ClaudeCode,
            "/scratch".into(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(lead, room).unwrap();
    finish_worker_request(&mut state, room, subject);
    state
        .set_agent_runtime_identity(
            lead,
            AgentRuntimeIdentity {
                launch_id: Some("launch-lead".into()),
                ..Default::default()
            },
        )
        .unwrap();
    state
        .observe_status(subject, RuntimeStatus::Idle, NOTICE_AT - 121_000)
        .unwrap();
    state
        .observe_status(lead, RuntimeStatus::Idle, NOTICE_AT)
        .unwrap();
    (state, room, subject, lead)
}

fn post_idle_notice(state: &mut BusState, lead: AgentId) -> RequestId {
    assert!(state
        .notify_idle_agents(10, 1, NOTICE_AT, &mut Default::default())
        .unwrap());
    state.queued_requests(lead)[0]
}

fn write_uncertain_notice(state: &mut BusState, id: RequestId) {
    use crate::messaging::model::SubmissionOutcome;

    state.begin_submission(id, "launch-lead", 5).unwrap();
    state
        .record_submission(
            id,
            SubmissionOutcome::Uncertain {
                message: "unknown write".into(),
            },
        )
        .unwrap();
}

#[test]
fn deleting_idle_notice_subject_retires_the_queue_entry_without_a_dashboard_attempt() {
    use crate::messaging::model::Author;

    let (mut state, room, subject, lead) = idle_notice_state();
    let id = post_idle_notice(&mut state, lead);
    let text = state.request(id).unwrap().prompt.text.clone();
    // Identical body without the structural link is ordinary Bus traffic.
    let generic = state
        .post_bus_notice(room, Some(lead), text.clone(), NOTICE_AT)
        .unwrap();
    state.set_draft_text(room, &text).unwrap();
    state.set_draft_recipients(room, [lead]).unwrap();
    let ordinary = state.submit_draft(room, NOTICE_AT).unwrap()[0];
    let mut expected = state.clone();
    expected.recover_idle_request(id, NOTICE_AT + 1).unwrap();
    let mut expected = serde_json::to_value(expected).unwrap();
    expected["requests"][id.0.to_string()]["failure_reason"] = "agent_deleted".into();
    let expected: BusState = serde_json::from_value(expected).unwrap();

    // Prove subject identity survives a save/load round trip before deletion.
    let mut state: BusState = serde_json::from_value(serde_json::to_value(state).unwrap()).unwrap();
    state.delete_agent(subject, NOTICE_AT + 1).unwrap();
    assert!(state
        .queued_requests(lead)
        .iter()
        .any(|id| state.request(*id).unwrap().prompt.id == generic));
    assert_eq!(
        state.request(ordinary).unwrap().prompt.author,
        Author::Developer
    );
    assert_eq!(state.request(ordinary).unwrap().phase, RequestPhase::Queued);
    let retired = state.request(id).unwrap();
    assert_eq!(retired.phase, RequestPhase::Abandoned);
    assert_eq!(retired.failure_reason.as_deref(), Some("agent_deleted"));
    assert!(!retired.uncertain_outcome);
    assert_eq!(state.queued_requests(lead).len(), 2);
    assert_eq!(report(&state), report(&expected));
}

#[test]
fn uncertain_idle_notice_is_deliberate_when_its_subject_is_deleted_before_closure() {
    use crate::messaging::model::UNCONFIRMED_SUBMISSION_MS;

    let (mut state, _, subject, lead) = idle_notice_state();
    let expected = report(&state);
    let id = post_idle_notice(&mut state, lead);
    write_uncertain_notice(&mut state, id);
    state.delete_agent(subject, NOTICE_AT + 1).unwrap();
    assert_eq!(state.request(id).unwrap().phase, RequestPhase::Submitting);
    state
        .fail_unconfirmed_submissions(NOTICE_AT + UNCONFIRMED_SUBMISSION_MS)
        .unwrap();
    let request = state.request(id).unwrap();
    assert_eq!(request.phase, RequestPhase::Abandoned);
    assert!(request.uncertain_outcome);
    assert_eq!(request.failure_reason.as_deref(), Some("agent_deleted"));
    assert!(state.agent(lead).unwrap().current_request.is_none());
    assert!(state.agent(lead).unwrap().actionable_error.is_none());
    assert_eq!(report(&state), expected);
}

#[test]
fn live_subject_uncertain_idle_notice_keeps_its_bus_row_after_later_deletion() {
    use crate::messaging::model::UNCONFIRMED_SUBMISSION_MS;

    let (mut state, _, subject, lead) = idle_notice_state();
    let id = post_idle_notice(&mut state, lead);
    write_uncertain_notice(&mut state, id);
    let deadline = NOTICE_AT + UNCONFIRMED_SUBMISSION_MS;
    state.fail_unconfirmed_submissions(deadline).unwrap();
    let request = state.request(id).unwrap();
    assert_eq!(request.phase, RequestPhase::Abandoned);
    assert!(request.uncertain_outcome);
    assert_eq!(
        request.failure_reason.as_deref(),
        Some("submission_unconfirmed")
    );
    let live_report = report(&state);
    assert_eq!(
        live_report["all"]["counts"]["bus_reasons"]["submission_unconfirmed"],
        1
    );
    assert_eq!(live_report["failures"][0]["request_id"], id.0);
    assert_eq!(live_report["failures"][0]["cause"], "bus");
    state.delete_agent(subject, deadline + 1).unwrap();
    assert_eq!(report(&state), live_report);
}

fn delivered_idle_notice() -> (BusState, AgentId, AgentId, RequestId) {
    use crate::messaging::model::{
        CallbackDisposition, CallbackEventKind, ProviderCallback, SubmissionOutcome,
    };

    let (mut state, _, subject, lead) = idle_notice_state();
    let id = post_idle_notice(&mut state, lead);
    state.begin_submission(id, "launch-lead", 5).unwrap();
    state
        .record_submission(
            id,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("session".into()),
                provider_turn_id: Some("turn".into()),
            },
        )
        .unwrap();
    let prompt = state.request(id).unwrap().prompt.clone();
    assert_eq!(
        state.accept_callback(ProviderCallback {
            callback_id: "idle-notice-started".into(),
            sequence: 6,
            occurred_at_ms: NOTICE_AT + 1,
            agent_id: lead,
            launch_id: "launch-lead".into(),
            provider_session_id: Some("session".into()),
            provider_turn_id: Some("turn".into()),
            provider_prompt_id: None,
            prompt_payload: Some(prompt.rendered_payload()),
            kind: CallbackEventKind::PromptStarted,
        }),
        CallbackDisposition::AcceptedBinding
    );
    (state, subject, lead, id)
}

fn assert_idle_notice_preserved(mut state: BusState, subject: AgentId, id: RequestId, stage: &str) {
    use crate::messaging::model::UNCONFIRMED_SUBMISSION_MS;

    let before = serde_json::to_value(state.request(id).unwrap()).unwrap();
    let dashboard = report(&state);
    state.delete_agent(subject, NOTICE_AT + 3).unwrap();
    state
        .fail_unconfirmed_submissions(NOTICE_AT + 3 + UNCONFIRMED_SUBMISSION_MS)
        .unwrap();
    assert_eq!(state.request(id).unwrap().delivery_stage(), stage);
    assert_eq!(
        serde_json::to_value(state.request(id).unwrap()).unwrap(),
        before
    );
    assert_eq!(report(&state), dashboard);
}

#[test]
fn deleting_idle_notice_subject_preserves_delivered_notice() {
    let (state, subject, _, id) = delivered_idle_notice();
    assert_idle_notice_preserved(state, subject, id, "delivered");
}

#[test]
fn deleting_idle_notice_subject_preserves_acknowledged_notice() {
    let (mut state, subject, lead, id) = delivered_idle_notice();
    let prompt = state.request(id).unwrap().prompt.id;
    state
        .answer_message(prompt, lead, "acknowledged".into(), NOTICE_AT + 2)
        .unwrap();
    assert_idle_notice_preserved(state, subject, id, "replied");
}

#[test]
fn uncertain_dialog_notice_keeps_its_existing_accounting_after_subject_deletion() {
    use crate::messaging::model::{
        DialogNoticeDelivery, DialogNoticeTo, UNCONFIRMED_SUBMISSION_MS,
    };

    let (mut state, _, subject, lead) = idle_notice_state();
    let Some(DialogNoticeTo::Orchestrator(id)) =
        state.post_blocked_notice(subject, NOTICE_AT).unwrap()
    else {
        panic!("expected a dialog notice to the orchestrator");
    };
    state
        .set_dialog_notice(subject, Some("blocked".into()))
        .unwrap();
    state
        .set_dialog_notice_to(subject, Some(DialogNoticeTo::Orchestrator(id)))
        .unwrap();
    let mut queued = state.clone();
    queued.delete_agent(subject, NOTICE_AT + 1).unwrap();
    assert!(queued.queued_requests(lead).is_empty());
    assert_eq!(
        queued.request(id).unwrap().failure_reason.as_deref(),
        Some("agent_deleted")
    );

    state.begin_dialog_notice_write(id, NOTICE_AT);
    write_uncertain_notice(&mut state, id);
    let delivery = state.request(id).unwrap().dialog_delivery.clone();
    assert!(matches!(
        delivery,
        Some(DialogNoticeDelivery::Unconfirmed { .. })
    ));
    state.delete_agent(subject, NOTICE_AT + 1).unwrap();
    state
        .fail_unconfirmed_submissions(NOTICE_AT + UNCONFIRMED_SUBMISSION_MS)
        .unwrap();
    assert_eq!(state.request(id).unwrap().dialog_delivery, delivery);
    assert_eq!(
        state.request(id).unwrap().failure_reason.as_deref(),
        Some("submission_unconfirmed")
    );
    assert_eq!(report(&state)["failures"][0]["cause"], "bus");
}

#[test]
fn a_stale_idle_notice_in_a_shared_write_does_not_hide_the_ordinary_failure() {
    use crate::messaging::model::UNCONFIRMED_SUBMISSION_MS;

    let (mut state, room, subject, lead) = idle_notice_state();
    let notice = post_idle_notice(&mut state, lead);
    let message = state
        .post_bus_notice(room, Some(lead), "ordinary Bus message".into(), NOTICE_AT)
        .unwrap();
    let ordinary = state.queued_requests(lead)[1];
    assert_eq!(state.request(ordinary).unwrap().prompt.id, message);
    assert!(state.coalesce_queue(notice).is_some());
    write_uncertain_notice(&mut state, notice);
    state.delete_agent(subject, NOTICE_AT + 1).unwrap();
    state
        .fail_unconfirmed_submissions(NOTICE_AT + UNCONFIRMED_SUBMISSION_MS)
        .unwrap();
    assert_eq!(
        state.request(notice).unwrap().failure_reason.as_deref(),
        Some("agent_deleted")
    );
    assert_eq!(
        state.request(ordinary).unwrap().failure_reason.as_deref(),
        Some("submission_unconfirmed")
    );
    let dashboard = report(&state);
    assert_eq!(dashboard["all"]["counts"]["attempts"], 2);
    assert_eq!(dashboard["all"]["counts"]["delivered"], 1);
    assert_eq!(
        dashboard["all"]["counts"]["bus_reasons"]["submission_unconfirmed"],
        1
    );
    assert_eq!(dashboard["failures"][0]["request_id"], ordinary.0);
    assert!(state.agent(lead).unwrap().actionable_error.is_some());
}

#[test]
fn a_roster_notice_keeps_its_queue_entry_while_one_subject_is_live() {
    use crate::messaging::model::RuntimeStatus;

    let (mut state, room, first, lead) = idle_notice_state();
    let second = state
        .create_agent(
            room,
            "second",
            Provider::ClaudeCode,
            "/scratch".into(),
            None,
        )
        .unwrap();
    finish_worker_request(&mut state, room, second);
    state
        .observe_status(second, RuntimeStatus::Idle, NOTICE_AT - 121_000)
        .unwrap();
    let expected = report(&state);
    assert!(state
        .notify_idle_agents(1, 10, NOTICE_AT, &mut Default::default())
        .unwrap());
    let roster = state.queued_requests(lead)[0];
    state.delete_agent(first, NOTICE_AT + 1).unwrap();
    assert_eq!(state.queued_requests(lead), &[roster]);
    state.delete_agent(second, NOTICE_AT + 2).unwrap();
    assert!(state.queued_requests(lead).is_empty());
    assert_eq!(
        state.request(roster).unwrap().failure_reason.as_deref(),
        Some("agent_deleted")
    );
    assert_eq!(report(&state), expected);
}

#[test]
fn deleting_a_worker_retires_its_queued_compaction_notice() {
    let (mut state, _, subject, lead) = idle_notice_state();
    let expected = report(&state);
    state.record_compaction(subject, NOTICE_AT - 1).unwrap();
    assert!(state.notify_compaction_limits(1, NOTICE_AT).unwrap());
    let id = state.queued_requests(lead)[0];
    state.delete_agent(subject, NOTICE_AT + 1).unwrap();
    assert!(state.queued_requests(lead).is_empty());
    assert_eq!(
        state.request(id).unwrap().failure_reason.as_deref(),
        Some("agent_deleted")
    );
    assert_eq!(report(&state), expected);
}

#[test]
fn rejected_idle_notice_retires_before_prompt_not_shown_expiry() {
    use crate::messaging::model::{SubmissionOutcome, QUEUED_STALL_MS};

    let (mut state, _, subject, lead) = idle_notice_state();
    let expected = report(&state);
    let id = post_idle_notice(&mut state, lead);
    state.begin_submission(id, "launch-lead", 5).unwrap();
    state
        .record_submission(
            id,
            SubmissionOutcome::DefinitelyRejected {
                message: crate::protocol::api::AGENT_PROMPT_NOT_SHOWN_MESSAGE.into(),
            },
        )
        .unwrap();
    assert_eq!(state.queued_requests(lead), &[id]);
    let mut live = state.clone();
    state.delete_agent(subject, NOTICE_AT + 1).unwrap();
    let deadline = NOTICE_AT + QUEUED_STALL_MS;
    assert_eq!(
        live.expire_stalled_queued_requests(deadline).unwrap(),
        vec![id]
    );
    assert_eq!(
        report(&live)["all"]["counts"]["bus_reasons"]["prompt_not_shown"],
        1
    );

    assert!(state
        .expire_stalled_queued_requests(deadline)
        .unwrap()
        .is_empty());
    assert_eq!(report(&state), expected);
}

#[test]
fn retiring_stale_notice_head_preserves_the_not_idle_hold_for_ordinary_work() {
    use crate::messaging::model::{SubmissionOutcome, QUEUED_STALL_MS};

    let (mut state, room, subject, lead) = idle_notice_state();
    let notice = post_idle_notice(&mut state, lead);
    state
        .post_bus_notice(room, Some(lead), "ordinary Bus message".into(), NOTICE_AT)
        .unwrap();
    let ordinary = state.queued_requests(lead)[1];

    for (boundary, message) in [
        (5, crate::protocol::api::AGENT_PROMPT_NOT_SHOWN_MESSAGE),
        (6, "Agent is not idle; prompt was not sent"),
    ] {
        assert!(state.coalesce_queue(notice).is_some());
        assert_eq!(state.request(ordinary).unwrap().group, Some(notice));
        state
            .begin_submission(notice, "launch-lead", boundary)
            .unwrap();
        state
            .record_submission(
                notice,
                SubmissionOutcome::DefinitelyRejected {
                    message: message.into(),
                },
            )
            .unwrap();
        assert_eq!(state.queued_requests(lead), &[notice, ordinary]);
    }
    let first = state
        .request(ordinary)
        .unwrap()
        .queued_rejection
        .as_ref()
        .unwrap();
    assert_eq!(first.reason, "prompt_not_shown");
    let deadline = first.at_ms + QUEUED_STALL_MS;
    assert_eq!(
        state.agent(lead).unwrap().delivery_rejection.as_deref(),
        Some("agent_not_idle")
    );

    state.delete_agent(subject, NOTICE_AT + 1).unwrap();
    assert_eq!(
        state.request(notice).unwrap().failure_reason.as_deref(),
        Some("agent_deleted")
    );
    assert_eq!(state.queued_requests(lead), &[ordinary]);
    assert!(state
        .expire_stalled_queued_requests(deadline)
        .unwrap()
        .is_empty());
    assert_eq!(state.queued_requests(lead), &[ordinary]);
    assert_eq!(state.request(ordinary).unwrap().phase, RequestPhase::Queued);
    assert_eq!(
        state.agent(lead).unwrap().delivery_rejection.as_deref(),
        Some("agent_not_idle")
    );
    let dashboard = report(&state);
    assert_eq!(dashboard["all"]["counts"]["attempts"], 2);
    assert_eq!(dashboard["all"]["counts"]["delivered"], 1);
    assert_eq!(dashboard["failures"], serde_json::json!([]));
}

#[test]
fn retiring_an_idle_notice_member_preserves_its_unknown_write_audit() {
    use crate::messaging::model::UNCONFIRMED_SUBMISSION_MS;

    let (mut state, room, subject, lead) = idle_notice_state();
    state
        .post_bus_notice(room, Some(lead), "ordinary Bus message".into(), NOTICE_AT)
        .unwrap();
    let ordinary = state.queued_requests(lead)[0];
    assert!(state
        .notify_idle_agents(10, 1, NOTICE_AT, &mut Default::default())
        .unwrap());
    let notice = state.queued_requests(lead)[1];
    assert!(state.coalesce_queue(ordinary).is_some());
    assert_eq!(state.request(notice).unwrap().group, Some(ordinary));
    write_uncertain_notice(&mut state, ordinary);
    assert!(state.request(ordinary).unwrap().uncertain_outcome);

    state.delete_agent(subject, NOTICE_AT + 1).unwrap();
    state
        .fail_unconfirmed_submissions(NOTICE_AT + UNCONFIRMED_SUBMISSION_MS)
        .unwrap();
    let retired = state.request(notice).unwrap();
    assert_eq!(retired.phase, RequestPhase::Abandoned);
    assert_eq!(retired.failure_reason.as_deref(), Some("agent_deleted"));
    assert!(retired.uncertain_outcome);
    let ordinary_request = state.request(ordinary).unwrap();
    assert_eq!(
        ordinary_request.failure_reason.as_deref(),
        Some("submission_unconfirmed")
    );
    assert!(ordinary_request.uncertain_outcome);

    let dashboard = report(&state);
    assert_eq!(dashboard["all"]["counts"]["attempts"], 2);
    assert_eq!(dashboard["all"]["counts"]["delivered"], 1);
    assert_eq!(
        dashboard["all"]["counts"]["bus_reasons"]["submission_unconfirmed"],
        1
    );
    assert_eq!(dashboard["failures"][0]["request_id"], ordinary.0);
}
