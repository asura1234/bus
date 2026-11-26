//! Orchestrator steer and queue markers clear when the lead turn ends, and
//! history never records stage ended with a null reply and no reason.
use super::*;
use crate::messaging::{control::Response, model::SendMode};
use std::sync::atomic::{AtomicU64, Ordering};

fn send(fixture: &mut Fixture, params: Value) -> Response {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    fixture.worker.control_response_with_events(
        &crate::messaging::control::Request {
            id: format!("marker-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
            method: "message.send".into(),
            params,
        },
        None,
    )
}

fn sent_request(response: &Response) -> RequestId {
    RequestId(
        response.result["request_ids"][0]
            .as_u64()
            .expect("request id"),
    )
}

fn orchestrator(fixture: &mut Fixture) -> AgentId {
    let mut state = fixture.worker.state.clone();
    let master = state.master_room().unwrap().id;
    let orch = state
        .create_agent(
            master,
            "orch",
            Provider::ClaudeCode,
            fixture.dir.clone(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(orch, fixture.room).unwrap();
    fixture.worker.save(state).unwrap();
    orch
}

fn history_row(fixture: &mut Fixture, id: RequestId) -> Value {
    let message = fixture.request(id).prompt.id.0;
    let response = fixture.worker.control_response_with_events(
        &crate::messaging::control::Request {
            id: format!("history-{message}"),
            method: "room.history".into(),
            params: json!({"room": fixture.room.0.to_string()}),
        },
        None,
    );
    assert!(response.ok, "{response:?}");
    response.result["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["prompt"]["id"].as_u64() == Some(message))
        .unwrap_or_else(|| panic!("message {message} missing from {response:?}"))["delivery"]
        ["requests"][0]
        .clone()
}

fn assert_history_settled(row: &Value) {
    let stage = row["stage"].as_str().unwrap_or("");
    let reason = row["reason"].as_str().filter(|reason| !reason.is_empty());
    let reply = row["reply"]["text"]
        .as_str()
        .filter(|text| !text.trim().is_empty());
    assert!(
        reply.is_some() || reason.is_some(),
        "history needs a reply or a named reason: {row}"
    );
    assert!(
        !(stage == "ended" && row["reply"].is_null() && reason.is_none()),
        "history left stage ended with a null reply and no reason: {row}"
    );
}

fn assert_status_settled(fixture: &mut Fixture, id: RequestId) {
    let status = fixture
        .worker
        .control_message(fixture.request(id).prompt.id)
        .unwrap();
    let request = &status["requests"][0];
    let reply = request["reply"]["text"]
        .as_str()
        .filter(|text| !text.trim().is_empty());
    let reason = request["reason"]
        .as_str()
        .filter(|reason| !reason.is_empty())
        .or(fixture.request(id).failure_reason.as_deref());
    assert!(
        reply.is_some() || reason.is_some(),
        "settlement recorded no reply and no failure cause: {status}"
    );
    assert!(
        fixture.request(id).history_mode_marker().is_none(),
        "badge survived settlement: {:?}",
        fixture.request(id).history_mode_marker()
    );
}

/// One steer and one queue while the lead is working. The steer reaches the
/// turn; the queue runs once the turn ends. Both then settle.
#[test]
fn orchestrator_steer_and_queue_settle_when_the_lead_turn_ends() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let orch = orchestrator(&mut fixture);
    let lead = running(&mut fixture, "fake:work 35 MODE_LEAD2", "1");
    let room = fixture.room.0.to_string();
    let author = orch.0.to_string();
    let steer = send(
        &mut fixture,
        json!({
            "room": room,
            "as": author,
            "to": ["builder"],
            "text": "MODE_STEER2",
            "mode": "steer",
        }),
    );
    assert!(steer.ok, "{steer:?}");
    let steer = sent_request(&steer);
    let queue = send(
        &mut fixture,
        json!({
            "room": room,
            "as": author,
            "to": ["builder"],
            "text": "MODE_QUEUE",
            "mode": "queue",
        }),
    );
    assert!(queue.ok, "{queue:?}");
    let queue = sent_request(&queue);

    assert_eq!(
        fixture.request(steer).history_mode_marker(),
        Some(" [pending]")
    );
    assert_eq!(
        fixture.request(queue).history_mode_marker(),
        Some(" [queued]")
    );
    assert_eq!(
        fixture.request(steer).history_send_mode(),
        Some(SendMode::Steer)
    );
    assert_eq!(
        fixture.request(queue).history_send_mode(),
        Some(SendMode::Queue)
    );

    fixture.deliver();
    assert_eq!(fixture.request(steer).group, Some(lead));
    assert_eq!(
        fixture.request(steer).history_mode_marker(),
        Some(" [pending]"),
        "an attempted write needs provider start evidence"
    );
    let steered_body = test_prompt_content(
        fixture
            .request(steer)
            .submitted_payload
            .as_deref()
            .expect("steer payload"),
    );
    fixture.started("1", &steered_body);
    fixture.consume();
    assert_eq!(fixture.request(steer).group, Some(lead));
    assert!(
        fixture.request(steer).trusted_start_bound,
        "steer must reach the running turn: {:?}",
        fixture.request(steer)
    );
    assert!(
        fixture.request(steer).history_mode_marker().is_none(),
        "a steer that reached the turn keeps no pending badge"
    );
    assert_eq!(
        fixture.request(queue).history_mode_marker(),
        Some(" [queued]"),
        "queue stays marked until the turn boundary"
    );
    assert_eq!(fixture.typed_content().len(), 2);

    fixture.stopped("1", "lead settled");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.worker.settle_requests().unwrap();
    assert_eq!(fixture.reply(steer).as_deref(), Some("lead settled"));
    assert_status_settled(&mut fixture, steer);
    let steer_row = history_row(&mut fixture, steer);
    assert_history_settled(&steer_row);
    assert_eq!(steer_row["reason"], "requester_reply");
    assert_eq!(
        fixture.request(queue).phase,
        RequestPhase::Queued,
        "queue does not close with the lead"
    );

    fixture.deliver();
    assert_eq!(fixture.typed_content().len(), 3);
    let queued_body = test_prompt_content(
        fixture
            .request(queue)
            .submitted_payload
            .as_deref()
            .expect("queue payload"),
    );
    assert!(
        queued_body.contains("MODE_QUEUE") && !fixture.typed().last().unwrap().1,
        "queue runs once, as its own turn: {:?}",
        fixture.typed_content()
    );
    fixture.started("2", &queued_body);
    fixture.consume();
    fixture.stopped("2", "queue settled");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(fixture.reply(queue).as_deref(), Some("queue settled"));
    assert_status_settled(&mut fixture, queue);
    let queue_row = history_row(&mut fixture, queue);
    assert_history_settled(&queue_row);
    assert_eq!(queue_row["reason"], "requester_reply");
    assert!(queue_row["reply"].is_null(), "{queue_row}");
    assert_status_settled(&mut fixture, steer);
    assert_history_settled(&history_row(&mut fixture, steer));
}

#[test]
fn a_silent_turn_has_a_reason_in_status_and_history() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut fixture, "no answer", "1");
    fixture.stopped("1", " ");
    fixture.consume();

    let status = fixture
        .worker
        .control_message(fixture.request(lead).prompt.id)
        .unwrap();
    for row in [
        status["requests"][0].clone(),
        history_row(&mut fixture, lead),
    ] {
        assert_eq!(row["stage"], "ended", "{row}");
        assert!(row["reply"].is_null(), "{row}");
        assert_eq!(row["reason"], "turn_ended_without_reply", "{row}");
    }
}

/// A steer the provider never claims closes with a named reason when the
/// lead ends, instead of stage ended and a null reply.
#[test]
fn an_unclaimed_steer_closes_with_a_reason_when_the_lead_ends() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let orch = orchestrator(&mut fixture);
    running(&mut fixture, "fake:work 35 MODE_LEAD2", "1");
    let room = fixture.room.0.to_string();
    let author = orch.0.to_string();
    let steer = send(
        &mut fixture,
        json!({
            "room": room,
            "as": author,
            "to": ["builder"],
            "text": "MODE_STEER2",
            "mode": "steer",
        }),
    );
    let steer = sent_request(&steer);
    fixture.deliver();
    fixture.stopped("1", "lead settled");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.worker.settle_requests().unwrap();
    assert_status_settled(&mut fixture, steer);
    assert_history_settled(&history_row(&mut fixture, steer));
    assert!(fixture.request(steer).history_mode_marker().is_none());
}

/// An interrupt ends the running turn. A steer that has not reached it stays
/// queued for a later turn and is not closed without a reply or a reason.
#[test]
fn an_interrupt_supersedes_a_pending_steer() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let orch = orchestrator(&mut fixture);
    let lead = running(&mut fixture, "fake:work 35 MODE_LEAD2", "1");
    let room = fixture.room.0.to_string();
    let author = orch.0.to_string();
    let steer = send(
        &mut fixture,
        json!({
            "room": room,
            "as": author,
            "to": ["builder"],
            "text": "MODE_STEER2",
            "mode": "steer",
        }),
    );
    let steer = sent_request(&steer);
    let interrupt = send(
        &mut fixture,
        json!({
            "room": room,
            "to": ["builder"],
            "text": "MODE_INTERRUPT",
        }),
    );
    assert!(interrupt.ok, "{interrupt:?}");
    let replacement = sent_request(&interrupt);
    assert_eq!(fixture.request(lead).group, Some(replacement));
    assert_eq!(fixture.request(steer).phase, RequestPhase::Queued);
    assert_eq!(fixture.request(steer).group, None);
    assert!(fixture.request(steer).failure_reason.is_none());
    assert_eq!(
        fixture.request(steer).history_mode_marker(),
        Some(" [pending]")
    );
    let row = history_row(&mut fixture, steer);
    assert_ne!(row["stage"], "ended");
    fixture.status(RuntimeStatus::Idle);
    fixture.deliver();
    let replacement_payload = fixture.typed().last().unwrap().0.clone();
    assert!(replacement_payload.ends_with("MODE_INTERRUPT"));
    assert!(!replacement_payload.contains("MODE_STEER2"));
    fixture.started("2", &replacement_payload);
    fixture.consume();
    fixture.stopped("2", "interrupted turn settled");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.deliver();
    assert!(
        fixture
            .typed_content()
            .last()
            .is_some_and(|(text, steered)| text.ends_with("MODE_STEER2") && !steered),
        "the pending steer runs in its own turn after the interrupt: {:?}",
        fixture.typed_content()
    );
}

/// A restart while a steer is still pending keeps it pending. It does not
/// settle the row as ended with a null reply.
#[test]
fn a_restart_keeps_a_pending_steer_unsettled() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let orch = orchestrator(&mut fixture);
    running(&mut fixture, "fake:work 35 MODE_LEAD2", "1");
    let room = fixture.room.0.to_string();
    let author = orch.0.to_string();
    let steer = send(
        &mut fixture,
        json!({
            "room": room,
            "as": author,
            "to": ["builder"],
            "text": "MODE_STEER2",
            "mode": "steer",
        }),
    );
    let steer = sent_request(&steer);
    let placeholder = Worker::open(
        fixture.dir.join("restart-placeholder"),
        Box::new(Native(fixture.native.clone())),
    )
    .unwrap();
    drop(std::mem::replace(&mut fixture.worker, placeholder));
    fixture.worker = Worker::open(
        fixture.dir.clone(),
        Box::new(Native(fixture.native.clone())),
    )
    .unwrap();
    let restored = fixture.request(steer);
    assert_eq!(restored.phase, RequestPhase::Queued);
    assert_eq!(restored.history_mode_marker(), Some(" [pending]"));
    assert!(restored.reply().is_none());
    assert!(restored.failure_reason.is_none());
    assert_ne!(restored.delivery_stage(), "ended");
    let row = history_row(&mut fixture, steer);
    assert_ne!(row["stage"], "ended");
}
