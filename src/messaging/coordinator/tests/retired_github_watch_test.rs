//! Backward compatibility must not revive the retired watcher.
use super::tests::support::test_prompt_content;
use crate::agents::AgentState;
use crate::messaging::coordinator::reply_delivery_support::Delivery;
use crate::messaging::model::{Author, Draft, Provider, RequestPhase};
use crate::messaging::storage::state_store::JsonStore;

#[tokio::test]
async fn retired_github_watch_queued_notice_is_closed_but_user_text_is_delivered() {
    let mut f = Delivery::new(&[Provider::Codex], false);
    let orch = f.add_orchestrator();
    let mut state = f.state().clone();
    let request = state
        .submit_message_from(
            f.work,
            Draft {
                text: "GitHub: owner/repo #42 ready for review".into(),
                files: vec![],
                recipient_ids: [orch].into(),
            },
            Author::Bus,
            10,
        )
        .unwrap()[0];
    JsonStore::new(f.dir.join("state.json"))
        .save(&state)
        .unwrap();
    f.reopen();
    f.status(orch, AgentState::Idle);
    f.tick();
    assert!(!f.typed(orch).iter().any(|text| text.starts_with("GitHub:")));
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
        Some("github_watch_retired")
    );
    let ordinary = f.send(orch, f.work, "GitHub: ordinary user text", false, None);
    f.tick();
    assert!(f
        .typed(orch)
        .iter()
        .any(|text| test_prompt_content(text) == "GitHub: ordinary user text"));
    assert_ne!(
        f.state().request(ordinary).unwrap().phase,
        RequestPhase::Abandoned
    );
}
