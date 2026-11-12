//! Cross-component tests consume a model fixture without constructing provider types.
use crate::messaging::model::{BusState, Provider, RoomId};
use std::path::Path;

pub(crate) fn mapped(root: &Path, outbound: &str) -> (BusState, String) {
    let mut state = BusState::new();
    let master = state.ensure_master_room();
    let room = state.create_room("work").unwrap();
    assert_eq!(
        room,
        RoomId(1),
        "numeric IDs deliberately overlap across fixtures"
    );
    let orch = state
        .create_agent(master, "orch", Provider::Codex, root.into(), None)
        .unwrap();
    state.bind_orchestrator(orch, room).unwrap();
    let before = state.clone();
    state
        .post_to_developer(master, orch, "[waiting-on-you] Choose".into(), vec![], 10)
        .unwrap();
    state.prepare_feishu_alerts(&before, root.to_str().unwrap(), "ou_developer", 11);
    let code = state.feishu.outbox.keys().next().unwrap().clone();
    state
        .complete_feishu_send(&code, outbound, "oc_dm")
        .unwrap();
    (state, code)
}
