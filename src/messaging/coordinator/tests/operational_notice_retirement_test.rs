use super::tests::support::test_prompt_content;
use crate::agents::AgentState;
use crate::messaging::coordinator::reply_delivery_support::Delivery;
use crate::messaging::model::{Author, BusState, Draft, Provider, RequestPhase};
use crate::messaging::storage::state_store::JsonStore;
use serde_json::json;

#[tokio::test]
async fn operational_notice_old_agents_are_not_given_upgrade_instruction_turns() {
    let mut f = Delivery::new(&[Provider::Codex], false);
    let agent = f.agents[0];
    let mut legacy = serde_json::to_value(f.state()).unwrap();
    legacy["agents"][agent.0.to_string()]["messaging_rules_version"] = json!(0);
    let state: BusState = serde_json::from_value(legacy).unwrap();
    JsonStore::new(f.dir.join("state.json"))
        .save(&state)
        .unwrap();
    f.reopen();
    f.status(agent, AgentState::Idle);
    f.tick();
    assert!(f
        .state()
        .requests()
        .all(|r| !r.prompt.text.starts_with("Bus messaging rules updated.")));
    assert!(!f
        .typed(agent)
        .iter()
        .any(|text| text.starts_with("Bus messaging rules updated.")));
}

#[tokio::test]
async fn operational_notice_pending_upgrade_turn_is_retired_without_touching_user_text() {
    let mut f = Delivery::new(&[Provider::Codex], false);
    let agent = f.agents[0];
    let mut state = f.state().clone();
    let request = state
        .submit_message_from(
            f.work,
            Draft {
                text:
                    "Bus messaging rules updated. Follow these rules for the rest of this session."
                        .into(),
                files: vec![],
                recipient_ids: [agent].into(),
            },
            Author::Bus,
            10,
        )
        .unwrap()[0];
    JsonStore::new(f.dir.join("state.json"))
        .save(&state)
        .unwrap();
    f.reopen();
    f.status(agent, AgentState::Idle);
    f.tick();
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
        Some("messaging_onboarding_retired")
    );
    assert!(!f
        .typed(agent)
        .iter()
        .any(|text| text.starts_with("Bus messaging rules updated.")));
    let user = f.send(
        agent,
        f.work,
        "Bus messaging rules updated. User-provided text",
        false,
        None,
    );
    f.tick();
    assert!(
        f.typed(agent)
            .iter()
            .any(|text| test_prompt_content(text)
                == "Bus messaging rules updated. User-provided text")
    );
    assert_ne!(
        f.state().request(user).unwrap().phase,
        RequestPhase::Abandoned
    );
}

#[tokio::test]
async fn operational_notice_retirement_does_not_create_a_replacement_worker_outcome() {
    let mut f = Delivery::new(&[Provider::Codex], false);
    let worker = f.agents[0];
    let orch = f.add_orchestrator();
    let mut state = f.state().clone();
    let request = state
        .submit_message_from(
            f.work,
            Draft {
                text:
                    "Bus messaging rules updated. Follow these rules for the rest of this session."
                        .into(),
                files: vec![],
                recipient_ids: [worker].into(),
            },
            Author::Bus,
            10,
        )
        .unwrap()[0];
    JsonStore::new(f.dir.join("state.json"))
        .save(&state)
        .unwrap();
    f.reopen();
    f.status(worker, AgentState::Idle);
    f.status(orch, AgentState::Idle);
    f.elapse(6_000);
    assert_eq!(
        f.state()
            .request(request)
            .unwrap()
            .failure_reason
            .as_deref(),
        Some("messaging_onboarding_retired")
    );
    let saved = serde_json::to_value(f.state()).unwrap();
    let event = format!("ended:{}", request.0);
    assert!(
        saved["worker_events"].get(&event).is_none(),
        "retired request created a pending replacement outcome: {}",
        saved["worker_events"]
    );
    assert!(
        f.typed(orch).is_empty(),
        "retired automation was re-notified: {:?}",
        f.typed(orch)
    );
}

#[tokio::test]
async fn operational_notice_legacy_startup_and_deletion_inputs_are_retired() {
    let mut f = Delivery::new(&[Provider::Codex], false);
    let orch = f.add_orchestrator();
    let mut state = f.state().clone();
    let mut requests = Vec::new();
    for text in [
        "BLOCKED worker (agent 7). Startup failed: no provider session.",
        "Deleted agent \"worker\". Terminal pane; left open.",
    ] {
        requests.push(
            state
                .submit_message_from(
                    f.work,
                    Draft {
                        text: text.into(),
                        files: vec![],
                        recipient_ids: [orch].into(),
                    },
                    Author::Bus,
                    10,
                )
                .unwrap()[0],
        );
    }
    JsonStore::new(f.dir.join("state.json"))
        .save(&state)
        .unwrap();
    f.reopen();
    f.status(orch, AgentState::Idle);
    f.tick();
    for request in requests {
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
            Some("operational_notice_retired")
        );
    }
    assert!(f.typed(orch).is_empty());
}
