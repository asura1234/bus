//! The waiter may print a receipt and swallow the reply; only action acknowledges facts.
use crate::agents::AgentState;
use crate::messaging::coordinator::reply_delivery_support::Delivery;
use crate::messaging::model::{AgentId, Author, Provider, RequestId};
use serde_json::{json, Value};

fn ready_while_orchestrator_busy(f: &mut Delivery, orchestrator: AgentId) -> RequestId {
    let worker = f.agents[0];
    f.status(orchestrator, AgentState::Working);
    f.tick();
    let request = f.send(worker, f.work, "verify #1602", false, Some(orchestrator));
    f.tick();
    let payload = f.typed(worker).last().unwrap().clone();
    f.start(worker, "ready-1602", &payload);
    f.stop(
        worker,
        "ready-1602",
        "READY FOR VERIFY #1602\nhead 870112624",
        json!([]),
        json!([]),
    );
    f.elapse(6_000);
    f.assert_reply(request, "READY FOR VERIFY #1602\nhead 870112624");
    // The CLI acknowledges a fully flushed result even if its caller only echoes "sent".
    f.call(
        "message.seen",
        json!({"request":request.0.to_string(),"as":orchestrator.0.to_string()}),
    );
    request
}

fn digests(f: &Delivery, orchestrator: AgentId) -> Vec<String> {
    f.typed(orchestrator)
        .into_iter()
        .filter(|text| text.starts_with("Room digest:"))
        .collect()
}

fn facts(f: &Delivery, orchestrator: AgentId) -> Vec<Value> {
    let state = serde_json::to_value(f.state()).unwrap();
    state["room_digests"][orchestrator.0.to_string()]["facts"]
        .as_object()
        .expect("durable digest facts")
        .values()
        .cloned()
        .collect()
}

#[tokio::test]
async fn room_digest_rescues_the_ready_final_a_shell_wrapper_consumed_once() {
    let mut f = Delivery::new(&[Provider::Codex], false);
    let orchestrator = f.add_orchestrator();
    ready_while_orchestrator_busy(&mut f, orchestrator);
    assert!(digests(&f, orchestrator).is_empty());
    f.status(orchestrator, AgentState::Idle);
    f.tick();
    let digests = digests(&f, orchestrator);
    assert_eq!(
        digests.len(),
        1,
        "transport-seen READY must still wake an idle orchestrator"
    );
    assert!(digests[0].contains("READY FOR VERIFY #1602"));
    assert!(
        digests[0].contains("age ") && digests[0].contains("next owner: orch"),
        "{}",
        digests[0]
    );
    let request = f
        .state()
        .requests()
        .find(|r| r.prompt.text == digests[0])
        .unwrap();
    assert_eq!(request.prompt.author, Author::Bus);
    assert!(request.queue_only);
    assert_eq!(request.room_id, f.work);
}

#[tokio::test]
async fn room_digest_waits_for_a_clean_turn_including_unbound_claude_work() {
    let mut f = Delivery::new(&[Provider::Codex], false);
    let orchestrator = f.add_orchestrator();
    ready_while_orchestrator_busy(&mut f, orchestrator);
    f.tick();
    assert!(digests(&f, orchestrator).is_empty());
    assert!(
        !f.state()
            .requests()
            .any(|r| r.prompt.text.starts_with("Room digest:")),
        "do not even inject while busy"
    );
    f.status(orchestrator, AgentState::Idle);
    f.tick();
    assert_eq!(digests(&f, orchestrator).len(), 1);
}

#[tokio::test]
async fn room_digest_delivery_is_not_ack_and_time_alone_never_rearms_it() {
    let mut f = Delivery::new(&[Provider::Codex], false);
    let orchestrator = f.add_orchestrator();
    ready_while_orchestrator_busy(&mut f, orchestrator);
    f.status(orchestrator, AgentState::Idle);
    f.tick();
    let payload = digests(&f, orchestrator).last().expect("digest").clone();
    f.start(orchestrator, "digest", &payload);
    f.stop(
        orchestrator,
        "digest",
        "DONE digest inspected",
        json!([]),
        json!([]),
    );
    f.elapse(3_600_001);
    for _ in 0..3 {
        f.tick();
    }
    assert_eq!(digests(&f, orchestrator).len(), 1);
    assert!(
        facts(&f, orchestrator)
            .iter()
            .any(|v| v["acknowledged_at_ms"].is_null()),
        "delivery/final alone cannot acknowledge READY"
    );
}

#[tokio::test]
async fn room_digest_survives_server_and_waiter_restart_with_its_watermark() {
    let mut f = Delivery::new(&[Provider::Codex], false);
    let orchestrator = f.add_orchestrator();
    ready_while_orchestrator_busy(&mut f, orchestrator);
    let before = facts(&f, orchestrator);
    f.reopen();
    assert_eq!(facts(&f, orchestrator), before);
    f.status(orchestrator, AgentState::Idle);
    f.tick();
    assert_eq!(digests(&f, orchestrator).len(), 1);
    let payload = digests(&f, orchestrator)[0].clone();
    f.start(orchestrator, "digest", &payload);
    f.stop(orchestrator, "digest", "noted", json!([]), json!([]));
    f.reopen();
    f.elapse(3_600_001);
    f.tick();
    assert_eq!(
        digests(&f, orchestrator).len(),
        1,
        "restart must not replay a delivered generation"
    );
}

#[tokio::test]
async fn room_digest_sending_the_next_task_acknowledges_only_that_workers_facts() {
    let mut f = Delivery::new(&[Provider::Codex, Provider::Codex], false);
    let orchestrator = f.add_orchestrator();
    ready_while_orchestrator_busy(&mut f, orchestrator);
    let other = f.agents[1];
    let other_request = f.send(other, f.work, "second item", false, Some(orchestrator));
    f.tick();
    let payload = f.typed(other).last().unwrap().clone();
    f.start(other, "other-done", &payload);
    f.stop(
        other,
        "other-done",
        "DONE second item",
        json!([]),
        json!([]),
    );
    f.elapse(6_000);
    f.call(
        "message.seen",
        json!({"request":other_request.0.to_string(),"as":orchestrator.0.to_string()}),
    );
    assert!(facts(&f, orchestrator)
        .iter()
        .any(|v| v["acknowledged_at_ms"].is_null()));
    let worker = f.agents[0];
    f.send(
        worker,
        f.work,
        "verification assigned; fix next",
        true,
        Some(orchestrator),
    );
    assert!(facts(&f, orchestrator)
        .iter()
        .filter(|v| v["agent"] == json!(worker))
        .all(|v| v["acknowledged_at_ms"].is_u64()));
    f.status(orchestrator, AgentState::Idle);
    f.tick();
    let digest = digests(&f, orchestrator);
    assert_eq!(digest.len(), 1);
    assert!(
        !digest[0].contains("READY FOR VERIFY #1602"),
        "an acted-on READY cannot wake again"
    );
    assert!(
        digest[0].contains("DONE second item"),
        "another worker's facts must remain pending"
    );
}

#[tokio::test]
async fn room_digest_does_not_fold_into_a_bound_master_turn() {
    let mut f = Delivery::new(&[Provider::Codex], false);
    let orchestrator = f.add_orchestrator();
    let developer = f.send(orchestrator, f.master, "developer task", false, None);
    f.tick();
    let payload = f.typed(orchestrator).last().unwrap().clone();
    f.start(orchestrator, "master", &payload);
    ready_while_orchestrator_busy(&mut f, orchestrator);
    assert!(digests(&f, orchestrator).is_empty());
    f.stop(
        orchestrator,
        "master",
        "developer report",
        json!([]),
        json!([]),
    );
    f.elapse(6_000);
    f.assert_reply(developer, "developer report");
    let payload = digests(&f, orchestrator)
        .last()
        .expect("separate digest turn")
        .clone();
    let digest = f
        .state()
        .requests()
        .find(|r| r.prompt.text == payload)
        .unwrap();
    assert_eq!(digest.group, None);
    assert!(!digest.steered);
    assert_eq!(digest.room_id, f.work);
}

#[tokio::test]
async fn room_digest_waits_for_an_unbound_start_even_when_idle_status_lags() {
    let mut f = Delivery::new(&[Provider::Codex], false);
    let orchestrator = f.add_orchestrator();
    ready_while_orchestrator_busy(&mut f, orchestrator);
    f.spool_start(orchestrator, "own-turn", "background notification");
    f.status(orchestrator, AgentState::Idle);
    f.tick();
    assert!(digests(&f, orchestrator).is_empty());
    assert!(!f
        .state()
        .requests()
        .any(|r| r.prompt.text.starts_with("Room digest:")));
    f.status(orchestrator, AgentState::Working);
    f.tick();
    f.stop(
        orchestrator,
        "own-turn",
        "own turn finished",
        json!([]),
        json!([]),
    );
    f.tick();
    assert_eq!(digests(&f, orchestrator).len(), 1);
}

#[tokio::test]
async fn room_digest_explicit_ack_is_durable_and_deleted_agent_facts_are_retained() {
    let mut f = Delivery::new(&[Provider::Codex], false);
    let orchestrator = f.add_orchestrator();
    ready_while_orchestrator_busy(&mut f, orchestrator);
    let before = facts(&f, orchestrator);
    let id = before
        .iter()
        .find(|v| v["text"].as_str().unwrap().starts_with("READY FOR VERIFY"))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    f.call("digest.ack", json!({"fact": id.to_string()}));
    let worker = f.agents[0];
    f.call(
        "agent.delete",
        json!({"agent": worker.0.to_string(), "confirm":true}),
    );
    f.reopen();
    assert!(f.state().agent(worker).is_none());
    let saved = serde_json::to_value(f.state()).unwrap();
    let tombstones = saved["room_digests"][orchestrator.0.to_string()]["tombstones"]
        .as_object()
        .unwrap();
    assert!(tombstones.values().any(|v| v["id"] == json!(id)
        && v["agent"] == json!(worker)
        && v["acknowledged_at_ms"].is_u64()));
    assert!(
        facts(&f, orchestrator).is_empty(),
        "acked payloads are discarded"
    );
    assert!(
        !f.state().requests().any(|r| r.agent_id == worker),
        "original request history was deleted"
    );
}

#[tokio::test]
async fn room_digest_does_not_rewake_after_the_orchestrator_assigned_a_verifier_for_ready() {
    let mut f = Delivery::new(&[Provider::Codex, Provider::Codex], false);
    let orchestrator = f.add_orchestrator();
    ready_while_orchestrator_busy(&mut f, orchestrator);
    // While still in its turn, the orchestrator handles READY the normal way:
    // it briefs a separate verifier rather than messaging the author again.
    let verifier = f.agents[1];
    f.send(
        verifier,
        f.work,
        "verify #1602 at 870112624",
        false,
        Some(orchestrator),
    );
    f.tick();
    f.status(orchestrator, AgentState::Idle);
    f.tick();
    assert_eq!(
        digests(&f, orchestrator),
        Vec::<String>::new(),
        "orchestrator re-woken for a READY it already dispatched"
    );
}

#[tokio::test]
async fn room_digest_broadcast_rule_does_not_acknowledge_a_ready_for_verify() {
    let mut f = Delivery::new(&[Provider::Codex, Provider::Codex], false);
    let orchestrator = f.add_orchestrator();
    ready_while_orchestrator_busy(&mut f, orchestrator);
    // The #1602 case: a rules broadcast is not verification action.
    f.call(
        "message.send",
        json!({"room":f.work.0.to_string(),"as":orchestrator.0.to_string(),
            "to":["all"],"text":"Never automate the real desktop.","queue":true}),
    );
    f.tick();
    let ready_unacked = facts(&f, orchestrator).iter().any(|fact| {
        fact["text"]
            .as_str()
            .is_some_and(|t| t.starts_with("READY FOR VERIFY"))
            && fact["acknowledged_at_ms"].is_null()
    });
    assert!(
        ready_unacked,
        "broadcast acknowledged READY: {:?}",
        facts(&f, orchestrator)
    );
}

#[tokio::test]
async fn room_digest_all_with_one_member_still_does_not_acknowledge_ready() {
    let mut f = Delivery::new(&[Provider::Codex], false);
    let orchestrator = f.add_orchestrator();
    ready_while_orchestrator_busy(&mut f, orchestrator);
    f.call("message.send", json!({"room":f.work.0.to_string(),"as":orchestrator.0.to_string(),"to":["all"],"text":"verify #1602","queue":true}));
    assert!(facts(&f, orchestrator).iter().any(|v| v["text"]
        .as_str()
        .is_some_and(|t| t.starts_with("READY"))
        && v["acknowledged_at_ms"].is_null()));
}
