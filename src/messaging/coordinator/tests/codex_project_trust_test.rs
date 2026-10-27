use super::*;

#[test]
fn untrusted_codex_restart_unconfirms_hooks_and_holds_both_pending_messages() {
    assert_restart_holds_pending(false);
}

#[test]
fn untrusted_codex_restart_holds_second_message_while_first_is_in_flight() {
    assert_restart_holds_pending(true);
}

fn assert_restart_holds_pending(submitted: bool) {
    let _lock = crate::utils::config::test_config_env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
    let home = dir.join("codex-home");
    std::fs::create_dir_all(&home).unwrap();
    let canonical = dir.canonicalize().unwrap();
    std::fs::write(
        home.join("config.toml"),
        format!(
            "[projects.{}]\ntrust_level = \"untrusted\"\n",
            serde_json::to_string(&canonical.display().to_string()).unwrap()
        ),
    )
    .unwrap();
    let first = queue(&mut worker, room, agent, "first pending message");
    if submitted {
        worker.submit_ready().unwrap();
    }
    let second = queue(&mut worker, room, agent, "second queued message");
    calls.lock().unwrap().clear();
    let before = [first, second].map(|id| worker.state.request(id).unwrap().clone());
    let identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
    drop(worker);
    let previous = std::env::var_os("CODEX_HOME");
    std::env::set_var("CODEX_HOME", &home);
    let opened = Worker::open(
        dir.clone(),
        Box::new(FakeTransport {
            replies: vec![].into(),
            calls: calls.clone(),
            state_path: dir.join("state.json"),
        }),
    );
    match previous {
        Some(value) => std::env::set_var("CODEX_HOME", value),
        None => std::env::remove_var("CODEX_HOME"),
    }
    let mut worker = opened.unwrap();
    let saved = worker.state.agent(agent).unwrap();
    assert!(
        !saved.hook_setup_confirmed,
        "restart falsely confirms hooks disabled by Codex trust"
    );
    assert!(saved
        .actionable_error
        .as_deref()
        .is_some_and(|e| e.contains("untrusted") && e.contains("/hooks")));
    assert_eq!(saved.runtime_identity, identity);
    worker.submit_ready().unwrap();
    for (id, expected) in [first, second].into_iter().zip(before) {
        assert_eq!(worker.state.request(id).unwrap(), &expected);
    }
    assert_eq!(
        worker.state.request(second).unwrap().phase,
        RequestPhase::Queued
    );
    assert!(calls.lock().unwrap().is_empty());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
