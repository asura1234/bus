use super::*;

#[test]
fn selector_label_ambiguous_as_does_not_block_an_exact_id_target() {
    let (mut w, room, agent, dir) = fixture();
    let other = w.state.create_room("other").unwrap();
    w.state
        .create_agent(other, "codex1", Provider::Codex, dir.clone(), None)
        .unwrap();
    let _ = room;
    for author in ["codex1", "ghost-author"] {
        let result = w.scoped_agent(&json!({"agent": agent.0.to_string(), "as": author}));
        assert_eq!(result, Ok(agent));
    }
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}
