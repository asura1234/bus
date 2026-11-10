use super::*;

#[test]
fn codex_cd_restart_preserves_trusted_target_from_an_untrusted_shell() {
    assert_cd_restart_trust(true, true);
}

#[test]
fn codex_cd_restart_holds_an_untrusted_target_from_a_trusted_shell() {
    assert_cd_restart_trust(false, true);
}

#[test]
fn codex_cd_restart_holds_unreadable_saved_options_instead_of_guessing_a_project() {
    assert_cd_restart_trust(true, false);
}

fn assert_cd_restart_trust(target_trusted: bool, valid_options: bool) {
    let _lock = crate::utils::config::test_config_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
    let target = dir.join("effective-project");
    let home = dir.join("codex-home");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let shell = dir.canonicalize().unwrap();
    let target = target.canonicalize().unwrap();
    let trust = |trusted| if trusted { "trusted" } else { "untrusted" };
    std::fs::write(
        home.join("config.toml"),
        format!(
            "[projects.{}]\ntrust_level = {:?}\n[projects.{}]\ntrust_level = {:?}\n",
            serde_json::to_string(&shell.display().to_string()).unwrap(),
            trust(!target_trusted),
            serde_json::to_string(&target.display().to_string()).unwrap(),
            trust(target_trusted),
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("callbacks/launch/launch-options.json"),
        if valid_options {
            serde_json::to_vec(&["--cd", target.to_str().unwrap()]).unwrap()
        } else {
            b"{".to_vec()
        },
    )
    .unwrap();
    let pending = queue(
        &mut worker,
        room,
        agent,
        "keep the effective project's work",
    );
    let before = worker.state.request(pending).unwrap().clone();
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
    let worker = opened.unwrap();
    let saved = worker.state.agent(agent).unwrap();
    assert_eq!(
        saved.hook_setup_confirmed,
        target_trusted && valid_options,
        "startup trust must follow saved --cd, not shell trust"
    );
    assert_eq!(
        saved.session_binding_invalidated,
        !target_trusted || !valid_options
    );
    assert_eq!(
        saved.actionable_error.is_none(),
        target_trusted && valid_options
    );
    if !valid_options {
        assert!(saved
            .actionable_error
            .as_deref()
            .unwrap()
            .contains("Invalid saved launch options"));
    }
    assert_eq!(worker.state.request(pending).unwrap(), &before);
    assert!(calls.lock().unwrap().is_empty());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

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
