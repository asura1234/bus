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
fn persistent_poll_timeout_preserves_status_and_recovery_keeps_unrelated_error() {
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
        RuntimeStatus::Idle
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
        RuntimeStatus::Idle
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
        RuntimeStatus::Idle
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

// Exercise the real transport classification, without opening a live socket.
fn native_error(
    error: crate::protocol::api::client::ApiClientError,
) -> Result<ResponseResult, TransportError> {
    use crate::messaging::native::BusTransport;
    use crate::protocol::api::client::ConnectionTarget;
    let mut error = Some(error);
    BusTransport::new(ConnectionTarget::SocketPath("unused".into())).request_using(
        Method::AgentListWithDetectionContext(
            crate::protocol::api::schema::AgentDetectionContexts { agents: Vec::new() },
        ),
        |_, _, budget| {
            assert_eq!(budget, Duration::from_secs(3));
            Err(error
                .take()
                .expect("poll must never retry inside the transport"))
        },
    )
}

#[test]
fn stalled_poll_preserves_all_rooms_statuses_requests_and_notice_state() {
    use crate::protocol::api::client::ApiClientError;
    let replies = (0..12)
        .map(|i| {
            native_error(ApiClientError::Io(std::io::Error::from(if i % 2 == 0 {
                std::io::ErrorKind::WouldBlock
            } else {
                std::io::ErrorKind::TimedOut
            })))
        })
        .collect();
    let (mut worker, active, room, dir, calls) = fixture(Provider::ClaudeCode, replies);
    let request = queue(&mut worker, room, active, "active work");
    let mut state = worker.state.clone();
    state.begin_submission(request, "launch", 0).unwrap();
    state
        .record_submission(
            request,
            SubmissionOutcome::Confirmed {
                provider_session_id: None,
                provider_turn_id: None,
            },
        )
        .unwrap();
    state
        .observe_status(active, RuntimeStatus::Working, io::now_ms())
        .unwrap();
    let other_room = state.create_room("other").unwrap();
    for (name, provider, status) in [
        ("idle", Provider::Codex, RuntimeStatus::Idle),
        ("blocked", Provider::Kimi, RuntimeStatus::Blocked),
    ] {
        let id = state
            .create_agent(other_room, name, provider, dir.clone(), None)
            .unwrap();
        state.observe_status(id, status, io::now_ms()).unwrap();
    }
    worker.save(state).unwrap();
    let before = worker.state.clone();
    let revision = worker.revision;
    for _ in 0..12 {
        worker.tick().unwrap();
        assert_eq!(
            worker.state, before,
            "a slow observation provides no new status or request evidence"
        );
        assert_eq!(
            worker.revision, revision,
            "no status-change persistence or notices during a stall"
        );
        assert!(worker.snapshot().error.is_none());
    }
    assert!(calls
        .lock()
        .unwrap()
        .iter()
        .all(|call| *call == "agent.list_with_detection_context"));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn native_disconnect_still_marks_unavailable_and_recovers() {
    use crate::protocol::api::client::ApiClientError;
    for kind in [
        std::io::ErrorKind::ConnectionRefused,
        std::io::ErrorKind::NotFound,
        std::io::ErrorKind::ConnectionReset,
        std::io::ErrorKind::BrokenPipe,
        std::io::ErrorKind::UnexpectedEof,
    ] {
        let mut replies: Vec<_> = (0..3)
            .map(|_| native_error(ApiClientError::Io(std::io::Error::from(kind))))
            .collect();
        replies.push(recovered());
        let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, replies);
        let request = queue(&mut worker, room, agent, "held until reconnect");
        for _ in 0..3 {
            worker.tick().unwrap();
        }
        assert_eq!(
            worker.state.agent(agent).unwrap().status,
            RuntimeStatus::Unavailable,
            "{kind:?}"
        );
        assert_eq!(
            worker.state.request(request).unwrap().phase,
            RequestPhase::Queued
        );
        worker.poll().unwrap();
        assert_eq!(
            worker.state.agent(agent).unwrap().status,
            RuntimeStatus::Idle
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
    let replies = (0..3)
        .map(|_| native_error(ApiClientError::EmptyResponse))
        .collect();
    let (mut worker, agent, _, dir, _) = fixture(Provider::ClaudeCode, replies);
    for _ in 0..3 {
        worker.tick().unwrap();
    }
    assert_eq!(
        worker.state.agent(agent).unwrap().status,
        RuntimeStatus::Unavailable
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn missing_agent_in_successful_poll_is_unavailable_immediately() {
    let (mut worker, agent, _, dir, _) = fixture(
        Provider::ClaudeCode,
        vec![Ok(ResponseResult::AgentList { agents: Vec::new() })],
    );
    worker.poll().unwrap();
    assert_eq!(
        worker.state.agent(agent).unwrap().status,
        RuntimeStatus::Unavailable
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn long_stall_recovery_delivers_queued_work_once() {
    let mut replies: Vec<_> = (0..12).map(|_| timeout()).collect();
    replies.extend([
        recovered(),
        Ok(ResponseResult::AgentPrompted {
            agent: native_info("pane", "claude", "session"),
        }),
    ]);
    let (mut worker, agent, room, dir, calls) = fixture(Provider::ClaudeCode, replies);
    let mut state = worker.state.clone();
    let request = state
        .submit_message_from(
            room,
            Draft {
                text: "deliver after recovery".into(),
                recipient_ids: [agent].into(),
                files: Vec::new(),
            },
            Author::Developer,
            io::now_ms().saturating_sub(60_000),
        )
        .unwrap()[0];
    worker.save(state).unwrap();
    for _ in 0..12 {
        worker.tick().unwrap();
        assert_eq!(
            worker.state.request(request).unwrap().phase,
            RequestPhase::Queued
        );
        assert_eq!(
            worker.state.agent(agent).unwrap().status,
            RuntimeStatus::Idle
        );
    }
    worker.tick().unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Active
    );
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| **call == "agent.prompt_if_idle")
            .count(),
        1
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn uncertain_protocol_failures_do_not_assert_native_server_death() {
    use crate::protocol::api::client::ApiClientError;
    for response in [
        Err(ApiClientError::Io(std::io::Error::other(
            "unknown observation failure",
        ))),
        Ok(json!({"id":"r", "error":{"code":"timeout", "message":"slow observation"}})),
        Ok(json!({"id":"r", "result":{"type":"ok"}})),
        Ok(json!({"invalid":"response"})),
    ] {
        let mut transport = crate::messaging::native::BusTransport::new(
            crate::protocol::api::client::ConnectionTarget::SocketPath("unused".into()),
        );
        let mut replies = Vec::new();
        for _ in 0..4 {
            // io::Error is not Clone.
            replies.push(transport.request_using(
                Method::AgentListWithDetectionContext(
                    crate::protocol::api::schema::AgentDetectionContexts { agents: Vec::new() },
                ),
                |_, _, _| match &response {
                    Ok(value) => Ok(value.clone()),
                    Err(_) => Err(ApiClientError::Io(std::io::Error::other(
                        "unknown observation failure",
                    ))),
                },
            ));
        }
        let (mut worker, agent, _, dir, _) = fixture(Provider::ClaudeCode, replies);
        for _ in 0..4 {
            worker.tick().unwrap();
        }
        assert_eq!(
            worker.state.agent(agent).unwrap().status,
            RuntimeStatus::Idle
        );
        assert!(worker.snapshot().error.is_none());
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
