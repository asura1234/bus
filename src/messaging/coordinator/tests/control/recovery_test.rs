//! Manual closure is an audit event, not a new provider failure or a replay.
use super::*;

const NATIVE_ERROR: &str =
    "Submit outcome unknown; no automatic retry: Composer changed during editor transfer";

fn enqueue(worker: &mut Worker, room: RoomId, agent: AgentId, text: &str) -> RequestId {
    worker
        .state
        .submit_message_with(
            room,
            Draft {
                text: text.into(),
                files: Vec::new(),
                recipient_ids: [agent].into(),
            },
            Author::Developer,
            100,
            false,
        )
        .unwrap()[0]
}

fn bind(worker: &mut Worker, agent: AgentId) {
    worker
        .state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                session_id: Some("session".into()),
                ..Default::default()
            },
        )
        .unwrap();
    worker.state.confirm_hook_setup(agent).unwrap();
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, 101)
        .unwrap();
}

fn start(worker: &mut Worker, request: RequestId) {
    worker
        .state
        .begin_submission(request, "launch", 102)
        .unwrap();
    worker
        .state
        .record_submission(
            request,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("session".into()),
                provider_turn_id: Some("1".into()),
            },
        )
        .unwrap();
    let agent = worker.state.request(request).unwrap().agent_id;
    let payload = worker.state.render_request_payload(request).unwrap();
    assert_eq!(
        worker.state.accept_callback(ProviderCallback {
            callback_id: "recovery-test-start".into(),
            sequence: 103,
            occurred_at_ms: 103,
            agent_id: agent,
            launch_id: "launch".into(),
            provider_session_id: Some("session".into()),
            provider_turn_id: Some("1".into()),
            provider_prompt_id: None,
            prompt_payload: Some(payload),
            kind: CallbackEventKind::PromptStarted,
        }),
        CallbackDisposition::AcceptedBinding
    );
}

fn recover(worker: &mut Worker, request: RequestId, actor: Option<AgentId>) -> Response {
    let mut params = json!({"request":request.0.to_string(),"confirm":true});
    if let Some(actor) = actor {
        params["as"] = actor.0.to_string().into();
    }
    call(
        worker,
        &format!("recover-{}", request.0),
        "request.recover",
        params,
    )
}

fn status(worker: &Worker, request: RequestId) -> Value {
    worker
        .control_message_at(worker.state.request(request).unwrap().prompt.id, 200)
        .unwrap()["requests"][0]
        .clone()
}

#[test]
fn manual_recovery_cause_and_actor_survive_reopen_for_queued_active_and_uncertain_work() {
    for mode in ["queued", "active", "uncertain", "native_reason"] {
        let (mut worker, room, agent, dir) = fixture();
        bind(&mut worker, agent);
        let request = enqueue(&mut worker, room, agent, mode);
        if mode != "queued" {
            worker
                .state
                .begin_submission(request, "launch", 102)
                .unwrap();
            worker
                .state
                .record_submission(
                    request,
                    if matches!(mode, "uncertain" | "native_reason") {
                        SubmissionOutcome::Uncertain {
                            message: NATIVE_ERROR.into(),
                        }
                    } else {
                        SubmissionOutcome::Confirmed {
                            provider_session_id: Some("session".into()),
                            provider_turn_id: Some("1".into()),
                        }
                    },
                )
                .unwrap();
        }
        if mode == "native_reason" {
            let mut snapshot = serde_json::to_value(&worker.state).unwrap();
            snapshot["requests"][request.0.to_string()]["failure_reason"] =
                json!("provider_error: saved native failure");
            worker.state = serde_json::from_value(snapshot).unwrap();
        }
        if mode == "queued" {
            worker
                .state
                .observe_status(agent, RuntimeStatus::Working, 102)
                .unwrap();
        }
        // Absence of --as represents the developer; explicit --as records an agent.
        let actor = (mode == "active").then_some(agent);
        let result = recover(&mut worker, request, actor);
        assert!(result.ok, "{mode}: {result:?}");
        let completed = worker
            .state
            .request(request)
            .unwrap()
            .completed_at_ms
            .unwrap();
        drop(worker);
        let mut reopened = Worker::open(dir.clone(), Box::new(NoTransport)).unwrap();
        let receipt = status(&reopened, request);
        assert_eq!(receipt["stage"], "abandoned", "{mode}");
        assert_eq!(
            receipt["reason"],
            if mode == "native_reason" {
                "provider_error: saved native failure"
            } else {
                "manual_recovery"
            },
            "{mode}"
        );
        assert_eq!(
            receipt["manual_recovery"],
            json!({
                "actor":actor.map_or(json!("human"), |id| json!({"agent":id})),
                "at_ms":completed,
            }),
            "{mode}"
        );
        if matches!(mode, "uncertain" | "native_reason") {
            assert_eq!(receipt["detail"], NATIVE_ERROR);
            assert_eq!(receipt["uncertain_outcome"], true);
        }
        reopened.submit_ready().unwrap(); // NoTransport panics if recovery replays input.
        let before = serde_json::to_value(reopened.state.request(request).unwrap()).unwrap();
        assert!(recover(&mut reopened, request, None).ok);
        assert_eq!(
            serde_json::to_value(reopened.state.request(request).unwrap()).unwrap(),
            before,
            "a repeated recovery preserves the original actor and clock"
        );
        drop(reopened);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn manual_group_recovery_preserves_replied_and_failed_members_and_closes_only_open_work() {
    for replied_lead in [false, true] {
        let (mut worker, room, agent, dir) = fixture();
        bind(&mut worker, agent);
        let lead = enqueue(&mut worker, room, agent, "lead");
        start(&mut worker, lead);
        worker
            .state
            .observe_status(agent, RuntimeStatus::Working, 103)
            .unwrap();
        let mut members = Vec::new();
        for text in [
            "already answered",
            "native failure",
            "unclaimed input",
            "open input",
        ] {
            let id = enqueue(&mut worker, room, agent, text);
            worker.state.begin_steering(id, lead).unwrap();
            worker
                .state
                .record_steering(
                    id,
                    SubmissionOutcome::Confirmed {
                        provider_session_id: Some("session".into()),
                        provider_turn_id: Some("1".into()),
                    },
                )
                .unwrap();
            members.push(id);
        }
        // Restore a pre-fix group with several uncertain writes. Current
        // admission holds the queue after the first one; recovery must still
        // handle the already persisted legacy group without replaying it.
        let mut snapshot = serde_json::to_value(&worker.state).unwrap();
        for id in &members[..3] {
            let member = &mut snapshot["requests"][id.0.to_string()];
            member["uncertain_outcome"] = json!(true);
            member["submission_error"] = json!(NATIVE_ERROR);
        }
        worker.state = serde_json::from_value(snapshot).unwrap();
        let message = worker.state.request(members[0]).unwrap().prompt.id;
        worker
            .state
            .answer_message(message, agent, "earlier reply".into(), 110)
            .unwrap();
        if replied_lead {
            let message = worker.state.request(lead).unwrap().prompt.id;
            worker
                .state
                .answer_message(message, agent, "lead reply".into(), 111)
                .unwrap();
        }
        // Reopen an authentic persisted group containing an earlier terminal failure.
        let mut snapshot = serde_json::to_value(&worker.state).unwrap();
        let prior = &mut snapshot["requests"][members[1].0.to_string()];
        prior["phase"] = json!("abandoned");
        prior["failure_reason"] = json!("submission_unconfirmed");
        prior["completed_at_ms"] = json!(112);
        worker.state = serde_json::from_value(snapshot).unwrap();
        worker
            .state
            .observe_status(agent, RuntimeStatus::Idle, 113)
            .unwrap();
        let preserved: Vec<_> = [members[0], members[1]]
            .into_iter()
            .chain(replied_lead.then_some(lead))
            .map(|id| {
                (
                    id,
                    serde_json::to_value(worker.state.request(id).unwrap()).unwrap(),
                )
            })
            .collect();
        assert!(recover(&mut worker, lead, None).ok);
        drop(worker);
        let mut reopened = Worker::open(dir.clone(), Box::new(NoTransport)).unwrap();
        for (id, before) in preserved {
            assert_eq!(
                serde_json::to_value(reopened.state.request(id).unwrap()).unwrap(),
                before,
                "prior outcome {id:?} must remain unchanged"
            );
        }
        let at = reopened
            .state
            .request(members[2])
            .unwrap()
            .completed_at_ms
            .unwrap();
        for id in members[2..]
            .iter()
            .copied()
            .chain((!replied_lead).then_some(lead))
        {
            let receipt = status(&reopened, id);
            assert_eq!(receipt["reason"], "manual_recovery", "{id:?}");
            assert_eq!(
                receipt["manual_recovery"],
                json!({"actor":"human","at_ms":at})
            );
        }
        assert_eq!(status(&reopened, members[2])["detail"], NATIVE_ERROR);
        assert_eq!(status(&reopened, members[2])["uncertain_outcome"], true);
        assert!(status(&reopened, members[3])["detail"].is_null());
        assert_eq!(status(&reopened, members[3])["uncertain_outcome"], false);
        assert_eq!(reopened.state.agent(agent).unwrap().current_request, None);
        reopened.submit_ready().unwrap();
        drop(reopened);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn manual_recovery_rejects_unknown_actor_without_closing_work() {
    let (mut worker, room, agent, dir) = fixture();
    let request = enqueue(&mut worker, room, agent, "queued");
    let result = recover(&mut worker, request, Some(AgentId(u64::MAX)));
    assert!(!result.ok);
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Queued
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn automatic_queued_recovery_does_not_claim_manual_actor_or_cause() {
    let (mut worker, room, agent, dir) = fixture();
    let request = enqueue(&mut worker, room, agent, "queued");
    worker.state.recover_idle_request(request, 200).unwrap();
    let record = serde_json::to_value(worker.state.request(request).unwrap()).unwrap();
    assert!(record.get("manual_recovery").is_none());
    assert_eq!(worker.state.request(request).unwrap().failure_reason, None);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn manual_recovery_remains_a_bus_caused_delivery_failure() {
    let class = crate::messaging::model::classify_request_failure("manual_recovery");
    assert_eq!(class, crate::messaging::model::RequestFailureClass::Bus);
    assert!(!class.is_deliberate());
    assert!(!class.is_external());
}

#[test]
fn manual_recovery_preserves_coalesced_submission_uncertainty() {
    let (mut worker, room, agent, dir) = fixture();
    bind(&mut worker, agent);
    let lead = enqueue(&mut worker, room, agent, "first queued input");
    let member = enqueue(&mut worker, room, agent, "second queued input");
    assert!(worker.state.coalesce_queue(lead).is_some());
    worker.state.begin_submission(lead, "launch", 102).unwrap();
    worker
        .state
        .record_submission(
            lead,
            SubmissionOutcome::Uncertain {
                message: NATIVE_ERROR.into(),
            },
        )
        .unwrap();
    assert!(recover(&mut worker, lead, None).ok);
    drop(worker);
    let mut reopened = Worker::open(dir.clone(), Box::new(NoTransport)).unwrap();
    for id in [lead, member] {
        let receipt = status(&reopened, id);
        assert_eq!(receipt["reason"], "manual_recovery", "{id:?}");
        assert_eq!(receipt["detail"], NATIVE_ERROR, "{id:?}");
        assert_eq!(
            receipt["uncertain_outcome"], true,
            "shared input for {id:?}"
        );
    }
    reopened.submit_ready().unwrap();
    drop(reopened);
    std::fs::remove_dir_all(dir).unwrap();
}
