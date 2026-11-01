use super::*;

fn timeout() -> Result<ResponseResult, TransportError> {
    Err(TransportError {
        code: None,
        message: std::io::Error::from(std::io::ErrorKind::WouldBlock).to_string(),
        definitely_rejected: false,
    })
}

fn recovered() -> Result<ResponseResult, TransportError> {
    let mut info = native_info("pane", "claude", "session");
    info.interactive_ready = true;
    Ok(ResponseResult::AgentList { agents: vec![info] })
}

#[test]
fn startup_poll_timeout_retries_without_status_or_composer_error_then_recovers() {
    let (mut worker, agent, room, dir, calls) = fixture(
        Provider::ClaudeCode,
        vec![timeout(), timeout(), recovered()],
    );
    let request = queue(&mut worker, room, agent, "retained during startup");
    for delay in [500, 1000] {
        assert!(worker.tick().is_ok(), "a poll timeout is a retryable tick");
        assert_eq!(worker.poll_interval(), Duration::from_millis(delay));
        assert_eq!(
            worker.state.agent(agent).unwrap().status,
            RuntimeStatus::Idle
        );
        assert!(worker.snapshot().error.is_none());
        assert_eq!(
            worker.state.request(request).unwrap().phase,
            RequestPhase::Queued
        );
    }
    worker.poll().unwrap();
    assert_eq!(worker.poll_interval(), Duration::from_millis(500));
    assert_eq!(
        worker.state.agent(agent).unwrap().status,
        RuntimeStatus::Idle
    );
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Queued
    );
    assert!(worker.snapshot().error.is_none());
    assert!(!calls.lock().unwrap().contains(&"agent.prompt_if_idle"));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn persistent_poll_failure_uses_agent_status_and_recovery_keeps_unrelated_error() {
    let (mut worker, agent, _, dir, _) = fixture(
        Provider::ClaudeCode,
        vec![timeout(), timeout(), timeout(), recovered()],
    );
    for _ in 0..3 {
        worker.tick().unwrap();
        assert!(worker.snapshot().error.is_none());
    }
    assert_eq!(worker.poll_interval(), Duration::from_secs(2));
    assert_eq!(
        worker.state.agent(agent).unwrap().status,
        RuntimeStatus::Unavailable
    );
    worker.error = Some("Unrelated storage failure".into());
    worker.poll().unwrap();
    assert_eq!(
        worker.state.agent(agent).unwrap().status,
        RuntimeStatus::Idle
    );
    assert_eq!(
        worker.snapshot().error.as_deref(),
        Some("Unrelated storage failure")
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn recovered_poll_resets_failure_budget_so_one_later_timeout_keeps_agent_status() {
    let (mut worker, agent, _, dir, _) = fixture(
        Provider::ClaudeCode,
        vec![timeout(), timeout(), timeout(), recovered(), timeout()],
    );
    for _ in 0..3 {
        worker.tick().unwrap();
    }
    assert_eq!(
        worker.state.agent(agent).unwrap().status,
        RuntimeStatus::Unavailable
    );
    worker.poll().unwrap();
    assert_eq!(worker.poll_interval(), Duration::from_millis(500));
    worker.tick().unwrap();
    assert_eq!(
        worker.state.agent(agent).unwrap().status,
        RuntimeStatus::Idle,
        "a single timeout after recovery starts a fresh failure budget"
    );
    assert_eq!(worker.poll_interval(), Duration::from_millis(500));
    assert!(worker.snapshot().error.is_none());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unexpected_agent_list_response_retries_without_composer_error() {
    let (mut worker, agent, _, dir, _) =
        fixture(Provider::ClaudeCode, vec![Ok(ResponseResult::Ok {})]);
    assert!(worker.poll().unwrap().is_none());
    assert_eq!(worker.poll_interval(), Duration::from_millis(500));
    assert_eq!(
        worker.state.agent(agent).unwrap().status,
        RuntimeStatus::Idle
    );
    assert!(worker.snapshot().error.is_none());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn long_poll_outage_caps_backoff_and_never_delivers_or_sets_composer_error() {
    let replies = (0..12).map(|_| timeout()).collect();
    let (mut worker, agent, room, dir, calls) = fixture(Provider::ClaudeCode, replies);
    let request = queue(&mut worker, room, agent, "held across outage");
    for _ in 0..12 {
        worker.tick().unwrap();
        assert!(worker.poll_interval() <= Duration::from_secs(2));
        assert!(worker.snapshot().error.is_none());
    }
    assert_eq!(worker.poll_interval(), Duration::from_secs(2));
    assert_eq!(
        worker.state.agent(agent).unwrap().status,
        RuntimeStatus::Unavailable
    );
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Queued
    );
    assert!(calls
        .lock()
        .unwrap()
        .iter()
        .all(|call| *call == "agent.list_with_detection_context"));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
