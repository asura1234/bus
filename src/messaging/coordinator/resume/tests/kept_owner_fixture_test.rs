use std::path::Path;

use super::NativeResumeContext;
use crate::messaging::{
    identity,
    model::{BusState, Provider},
    storage::state_store::JsonStore,
};

pub(crate) fn save(root: &Path, deleted: bool) -> (String, NativeResumeContext) {
    std::fs::create_dir_all(root).unwrap();
    let mut state = BusState::new();
    let room = state.create_room("work").unwrap();
    // The server saves Claude as the harness kind, including for claude-ds.
    let provider = if deleted {
        Provider::ClaudeDs
    } else {
        Provider::Codex
    };
    let agent = state
        .create_agent(room, "saved-worker", provider, root.into(), None)
        .unwrap();
    if deleted {
        state.prepare_delete_agent(agent).unwrap();
        state.delete_agent(agent, 1).unwrap();
    }
    JsonStore::new(root.join("state.json"))
        .save(&state)
        .unwrap();
    let context = NativeResumeContext {
        root: Some(root.into()),
        session_name: Some("bus".into()),
        config_root: root.join("herdr-config"),
        project: root.into(),
        binary: std::env::current_exe().unwrap(),
    };
    (identity::managed_name(room, agent), context)
}
