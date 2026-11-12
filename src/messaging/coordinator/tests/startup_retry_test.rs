use super::super::launch_metrics_tests::{ledger, seed_launch};
use super::*;

#[test]
fn codex_transient_bootstrap_exit_retries_without_closing_queued_work() {
    let (mut worker, agent, room, dir) = pending(Provider::Codex);
    let request = queue(&mut worker, room, agent, "first queued work");
    let calls = Arc::new(Mutex::new(0));
    worker.transport = Box::new(FailedLaunchNative {
        state_path: dir.join("state.json"),
        screen: Some("Error: account/read failed during TUI bootstrap: workspace routing discovery failed (-32603)".into()),
        exited: true,
        retry_calls: Some(calls.clone()),
        ..Default::default()
    });
    let start = worker
        .state
        .agent(agent)
        .unwrap()
        .startup
        .as_ref()
        .unwrap()
        .started_at_ms;
    seed_launch(&mut worker, agent, 0);
    worker.poll().unwrap();
    assert_eq!(ledger(&worker)["1"]["outcome"], "pending");
    worker
        .cleanup_failed_startups(&[], &mpsc::channel().0, start + 30_000)
        .unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Queued,
        "a proven transient bootstrap exit must retain the first message"
    );
    let late = queue(
        &mut worker,
        room,
        agent,
        "second message arrives during backoff",
    );
    worker
        .cleanup_failed_startups(&[], &mpsc::channel().0, start + 35_000)
        .unwrap();
    assert_eq!(*calls.lock().unwrap(), 1);
    assert_eq!(
        worker
            .state
            .request(request)
            .unwrap()
            .provider_retry_attempts,
        1
    );
    assert_eq!(
        worker.state.request(late).unwrap().provider_retry_attempts,
        1
    );
    let agent_state = worker.state.agent(agent).unwrap();
    assert!(!agent_state.startup.as_ref().unwrap().reported);
    let snapshot = agent_state
        .exit_snapshot
        .as_ref()
        .expect("save the exited launch screen before retry");
    assert!(std::fs::read_to_string(snapshot.path.as_ref().unwrap())
        .unwrap()
        .contains("workspace routing discovery failed"));
    let mut ready = pending_info(&worker, agent);
    ready.interactive_ready = true;
    ready.launch_pending = false;
    worker
        .cleanup_failed_startups(&[ready], &mpsc::channel().0, start + 35_100)
        .unwrap();
    assert!(worker.state.agent(agent).unwrap().startup.is_none());
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Queued
    );
    let calls = Arc::default();
    let mut ready = pending_info(&worker, agent);
    ready.interactive_ready = true;
    ready.native_ready = true;
    ready.launch_pending = false;
    worker.transport = Box::new(FakeTransport {
        replies: vec![
            Ok(ResponseResult::AgentList {
                agents: vec![ready.clone()],
            }),
            Ok(ResponseResult::AgentInfo { agent: ready }),
            Ok(ResponseResult::Ok {}),
        ]
        .into(),
        calls: Arc::clone(&calls),
        state_path: dir.join("state.json"),
    });
    worker.poll().unwrap();
    worker.submit_ready().unwrap();
    worker.submit_ready().unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Submitting
    );
    assert_eq!(ledger(&worker)["1"]["outcome"], "usable");
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|method| **method == "agent.prompt_if_unbound_at_generation")
            .count(),
        1
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn codex_bootstrap_retry_excludes_auth_config_other_providers_and_unproven_exit() {
    for (provider, exited, screen) in [
        (Provider::Codex, true, "account/read failed during TUI bootstrap: workspace routing discovery unauthorized (401)"),
        (Provider::Codex, true, "account/read failed during TUI bootstrap: workspace routing requires a ChatGPT account id"),
        (Provider::Codex, true, "account/read failed during TUI bootstrap: configuration changed during workspace routing discovery; retry account/read"),
        (Provider::Codex, true, "Error: account/read failed during TUI bootstrap: workspace routing discovery failed\nscratch% codex\nError: invalid configuration: workspace backend must use an HTTPS origin"),
        (Provider::Codex, false, "account/read failed during TUI bootstrap: workspace routing discovery failed"),
        (Provider::ClaudeCode, true, "account/read failed during TUI bootstrap: workspace routing discovery failed"),
    ] {
        let (mut worker, agent, room, dir) = pending(provider);
        let request = queue(&mut worker, room, agent, "preserve uncertain input");
        let calls = Arc::new(Mutex::new(0));
        worker.transport = Box::new(FailedLaunchNative {
            state_path: dir.join("state.json"), screen: Some(screen.into()), exited,
            retry_calls: Some(calls.clone()), ..Default::default()
        });
        let start = worker.state.agent(agent).unwrap().startup.as_ref().unwrap().started_at_ms;
        worker.cleanup_failed_startups(&[], &mpsc::channel().0, start + 30_000).unwrap();
        worker.cleanup_failed_startups(&[], &mpsc::channel().0, start + 60_000).unwrap();
        assert_eq!(*calls.lock().unwrap(), 0, "{provider:?}: {screen}");
        assert_eq!(worker.state.request(request).unwrap().failure_reason.as_deref(), Some("agent_startup_failed"));
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn fast_codex_bootstrap_exit_captures_screen_and_reports_exit_instead_of_timeout() {
    let (mut worker, agent, room, dir) = pending(Provider::Codex);
    let request = queue(&mut worker, room, agent, "never submitted");
    worker.transport = Box::new(FailedLaunchNative {
        state_path: dir.join("state.json"), exited: true,
        screen: Some("account/read failed during TUI bootstrap: workspace routing discovery unauthorized (401)".into()),
        ..Default::default()
    });
    let start = worker
        .state
        .agent(agent)
        .unwrap()
        .startup
        .as_ref()
        .unwrap()
        .started_at_ms;
    worker
        .cleanup_failed_startups(&[], &mpsc::channel().0, start + 30_000)
        .unwrap();
    let a = worker.state.agent(agent).unwrap();
    let error = a.actionable_error.as_deref().unwrap();
    assert!(error.contains("launch ended"), "{error}");
    assert!(!error.contains("within 30 seconds"), "{error}");
    assert!(error.contains("codex login"), "{error}");
    assert!(a
        .exit_snapshot
        .as_ref()
        .unwrap()
        .path
        .as_ref()
        .unwrap()
        .is_file());
    assert_eq!(
        worker
            .state
            .request(request)
            .unwrap()
            .provider_retry_attempts,
        0
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn codex_bootstrap_retry_budget_and_backoff_survive_coordinator_restart() {
    let (mut worker, agent, room, dir) = pending(Provider::Codex);
    let request = queue(
        &mut worker,
        room,
        agent,
        "first message retained through retries",
    );
    let calls = Arc::new(Mutex::new(0));
    let native = || FailedLaunchNative {
        state_path: dir.join("state.json"),
        exited: true,
        screen: Some(
            "account/read failed during TUI bootstrap: workspace routing discovery timed out"
                .into(),
        ),
        retry_calls: Some(calls.clone()),
        ..Default::default()
    };
    worker.transport = Box::new(native());
    let start = worker
        .state
        .agent(agent)
        .unwrap()
        .startup
        .as_ref()
        .unwrap()
        .started_at_ms;
    seed_launch(&mut worker, agent, start + 30_000);
    worker
        .cleanup_failed_startups(&[], &mpsc::channel().0, start + 30_000)
        .unwrap();
    drop(worker);
    let mut worker = Worker::open(dir.clone(), Box::new(native())).unwrap();
    worker
        .cleanup_failed_startups(&[], &mpsc::channel().0, start + 34_999)
        .unwrap();
    assert_eq!(*calls.lock().unwrap(), 0);
    worker
        .cleanup_failed_startups(&[], &mpsc::channel().0, start + 35_000)
        .unwrap();
    assert_eq!(*calls.lock().unwrap(), 1);
    worker
        .cleanup_failed_startups(&[], &mpsc::channel().0, start + 35_100)
        .unwrap();
    worker
        .cleanup_failed_startups(&[], &mpsc::channel().0, start + 55_099)
        .unwrap();
    assert_eq!(*calls.lock().unwrap(), 1);
    worker
        .cleanup_failed_startups(&[], &mpsc::channel().0, start + 55_100)
        .unwrap();
    assert_eq!(*calls.lock().unwrap(), 2);
    worker
        .cleanup_failed_startups(&[], &mpsc::channel().0, start + 55_200)
        .unwrap();
    worker
        .cleanup_failed_startups(&[], &mpsc::channel().0, start + 120_000)
        .unwrap();
    assert_eq!(*calls.lock().unwrap(), 2);
    assert_eq!(ledger(&worker)["1"]["outcome"], "failed");
    assert_eq!(ledger(&worker)["1"]["cause"], "unavailable_at_launch");
    let request = worker.state.request(request).unwrap();
    assert_eq!(request.provider_retry_attempts, 2);
    assert_eq!(
        request.failure_reason.as_deref(),
        Some("provider_error: codex backend unreachable (account/read)")
    );
    let error = worker
        .state
        .agent(agent)
        .unwrap()
        .actionable_error
        .as_deref()
        .unwrap();
    assert!(error.contains("after 3 tries"), "{error}");
    assert!(error.contains("launch ended"), "{error}");
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn codex_bootstrap_retry_never_replays_uncertain_launch_input() {
    let (mut worker, agent, room, dir) = pending(Provider::Codex);
    let request = queue(&mut worker, room, agent, "must not be duplicated");
    let calls = Arc::new(Mutex::new(0));
    let native = || FailedLaunchNative {
        state_path: dir.join("state.json"),
        exited: true,
        uncertain_retry: true,
        screen: Some(
            "account/read failed during TUI bootstrap: workspace routing discovery failed".into(),
        ),
        retry_calls: Some(calls.clone()),
        ..Default::default()
    };
    worker.transport = Box::new(native());
    let start = worker
        .state
        .agent(agent)
        .unwrap()
        .startup
        .as_ref()
        .unwrap()
        .started_at_ms;
    worker
        .cleanup_failed_startups(&[], &mpsc::channel().0, start + 30_000)
        .unwrap();
    let _ = worker.cleanup_failed_startups(&[], &mpsc::channel().0, start + 35_000);
    assert_eq!(*calls.lock().unwrap(), 1);
    drop(worker);
    let mut worker = Worker::open(dir.clone(), Box::new(native())).unwrap();
    worker
        .cleanup_failed_startups(&[], &mpsc::channel().0, start + 35_100)
        .unwrap();
    worker
        .cleanup_failed_startups(&[], &mpsc::channel().0, start + 90_000)
        .unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        1,
        "unknown launch acceptance must not authorize another command"
    );
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Queued
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rejected_saved_session_startup_does_not_archive_the_old_conversation() {
    let (mut worker, agent, _, dir) = pending(Provider::Codex);
    let mut state = worker.state.clone();
    let mut identity = state.agent(agent).unwrap().runtime_identity.clone();
    identity.session_id = Some("saved-conversation".into());
    state.set_agent_runtime_identity(agent, identity).unwrap();
    worker.save(state).unwrap();
    worker.transport = Box::new(FailedLaunchNative {
        state_path: dir.join("state.json"),
        exited: true,
        screen: Some("old conversation text must not be archived".into()),
        ..Default::default()
    });
    let mut info = pending_info(&worker, agent);
    info.agent = None;
    info.launch_pending = false;
    info.launch_input_error = Some("launch rejected before Enter".into());
    worker
        .cleanup_failed_startups(&[info], &mpsc::channel().0, io::now_ms())
        .unwrap();
    assert!(!dir.join("exit-snapshots").exists());
    let error = worker
        .state
        .agent(agent)
        .unwrap()
        .actionable_error
        .as_deref()
        .unwrap();
    assert!(error.contains("Conversation screen omitted"));
    assert!(!error.contains("old conversation text"));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
