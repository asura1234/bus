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
    assert!(f
        .typed(agent)
        .iter()
        .any(|text| text == "Bus messaging rules updated. User-provided text"));
    assert_ne!(
        f.state().request(user).unwrap().phase,
        RequestPhase::Abandoned
    );
}
