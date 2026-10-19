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
