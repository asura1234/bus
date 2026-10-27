//! Retired workflow automation must not wake an otherwise idle orchestrator.
use super::tests::support::test_prompt_content;
use crate::agents::AgentState;
use crate::messaging::coordinator::reply_delivery_support::Delivery;
use crate::messaging::model::{Author, BusState, Draft, Provider, RequestPhase};
use crate::messaging::storage::state_store::JsonStore;
use serde_json::json;

#[tokio::test]
async fn retired_room_digest_does_not_repackage_a_seen_worker_reply_as_another_turn() {
    let mut f = Delivery::new(&[Provider::Codex], false);
    let orch = f.add_orchestrator();
    let worker = f.agents[0];
    f.status(orch, AgentState::Working);
    f.tick();
    let request = f.send(worker, f.work, "verify result", false, Some(orch));
    f.tick();
    let payload = f.typed(worker).last().unwrap().clone();
    f.start(worker, "ready", &payload);
    f.stop(
        worker,
        "ready",
        "READY FOR VERIFY #42\nhead 1234567",
        json!([]),
        json!([]),
    );
    f.elapse(6_000);
    f.assert_reply(request, "READY FOR VERIFY #42\nhead 1234567");
    f.call(
        "message.seen",
        json!({"request":request.0.to_string(),"as":orch.0.to_string()}),
    );
    f.status(orch, AgentState::Idle);
    f.tick();
    assert!(!f
        .typed(orch)
        .iter()
        .any(|text| text.starts_with("Room digest:")));
    f.reopen();
    f.status(orch, AgentState::Idle);
    f.elapse(60_000);
    assert!(!f
        .typed(orch)
        .iter()
        .any(|text| text.starts_with("Room digest:")));
}

#[tokio::test]
async fn retired_room_digest_queued_legacy_message_is_closed_without_typing_after_restart() {
    let mut f = Delivery::new(&[Provider::Codex], false);
    let orch = f.add_orchestrator();
    let mut state = f.state().clone();
    let request = state
        .submit_message_from(
            f.work,
            Draft {
                text: "Room digest: 1 unacknowledged facts in room 2.".into(),
                files: vec![],
                recipient_ids: [orch].into(),
            },
            Author::Bus,
            10,
        )
        .unwrap()[0];
    let mut legacy = serde_json::to_value(state).unwrap();
    legacy["room_digests"] = json!({orch.0.to_string():{
        "armed":true,"last_digest_request":request.0,"watermark":0,"pending_watermark":99,
        "facts":{"idle:7:1":{"id":99,"source_id":7,"room":f.work.0,"agent":f.agents[0].0,
        "agent_name":"worker","at_ms":10,"text":"Idle with no next task",
        "next_action":"assign work","acknowledged_at_ms":null}}
    }});
    let state: BusState = serde_json::from_value(legacy).unwrap();
    JsonStore::new(f.dir.join("state.json"))
        .save(&state)
        .unwrap();
    f.reopen();
    f.status(orch, AgentState::Idle);
    f.tick();
    assert!(!f
        .typed(orch)
        .iter()
        .any(|text| text.starts_with("Room digest:")));
    assert_eq!(
        f.state().request(request).unwrap().phase,
        RequestPhase::Abandoned
    );
    assert_eq!(
        f.state()
            .request(request)
            .unwrap()
            .failure_reason
            .as_deref(),
        Some("room_digest_retired")
    );
    assert!(serde_json::to_value(f.state())
        .unwrap()
        .get("room_digests")
        .is_none());
    let ordinary = f.send(orch, f.work, "Room digest: ordinary user text", false, None);
    f.tick();
    assert!(f
        .typed(orch)
        .iter()
        .any(|text| test_prompt_content(text) == "Room digest: ordinary user text"));
    assert_ne!(
        f.state().request(ordinary).unwrap().phase,
        RequestPhase::Abandoned
    );
}
