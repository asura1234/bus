use super::*;

fn directory_fixture(
    provider: Provider,
    status: schema::AgentStatus,
) -> (Worker, AgentId, PathBuf, PathBuf) {
    let native = match provider {
        Provider::ClaudeCode | Provider::ClaudeDs => "claude",
        Provider::Codex => "codex",
        Provider::Cursor => "cursor",
        Provider::Grok => "grok",
        Provider::Kimi => "kimi",
        #[cfg(feature = "dev-tools")]
        Provider::Fake => "fake",
    };
    let mut info = native_info("pane", native, "session");
    info.interactive_ready = true;
    info.agent_status = status;
    let (mut worker, agent, _, dir, _) = fixture(provider, Vec::new());
    worker.transport = Box::new(DirectoryTransport(info));
    let cwd = dir.join("worktree");
    std::fs::create_dir(&cwd).unwrap();
    let mut state = worker.state.clone();
    state
        .update_agent_runtime_metadata(agent, cwd.clone(), None)
        .unwrap();
    worker.save(state).unwrap();
    (worker, agent, dir, cwd)
}

struct DirectoryTransport(schema::AgentInfo);

impl Transport for DirectoryTransport {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::AgentList(_) | Method::AgentListWithDetectionContext(_) => {
                Ok(ResponseResult::AgentList {
                    agents: vec![self.0.clone()],
                })
            }
            _ => Ok(ResponseResult::Ok {}),
        }
    }
}

fn poll_until(worker: &mut Worker, agent: AgentId, predicate: impl Fn(&RoomAgent) -> bool) {
    let started = std::time::Instant::now();
    loop {
        worker.poll().unwrap();
        if predicate(worker.state.agent(agent).unwrap()) {
            return;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "directory observation never reached expected state"
        );
        std::thread::yield_now();
    }
}

#[test]
fn deleted_working_directory_surfaces_recovery_action_for_every_provider() {
    for (provider, status, expected) in [Provider::Codex, Provider::ClaudeCode, Provider::Cursor]
        .into_iter()
        .flat_map(|provider| {
            [
                (provider, schema::AgentStatus::Idle, RuntimeStatus::Idle),
                (
                    provider,
                    schema::AgentStatus::Working,
                    RuntimeStatus::Working,
                ),
            ]
        })
    {
        let (mut worker, agent, dir, cwd) = directory_fixture(provider, status);
        worker.poll().unwrap();
        assert!(worker
            .state
            .agent(agent)
            .unwrap()
            .actionable_error
            .is_none());
        std::fs::remove_dir(&cwd).unwrap();
        poll_until(&mut worker, agent, |a| a.actionable_error.is_some());
        let observed = worker.state.agent(agent).unwrap();
        let error = observed
            .actionable_error
            .as_deref()
            .expect("missing cwd must be visible on the agent");
        assert!(error.contains(cwd.to_str().unwrap()), "{error}");
        assert!(
            error.contains("replace the agent") && error.contains("--pwd"),
            "{error}"
        );
        assert!(
            observed.details_disclosed,
            "the warning must not hide behind collapsed details"
        );
        assert_eq!(
            observed.status, expected,
            "do not stop or restart the provider"
        );
        assert!(observed.working_directory_error.is_some());
        assert_eq!(observed.shown_status(), RuntimeStatus::Unavailable);
        assert_eq!(
            JsonStore::new(dir.join("state.json"))
                .load()
                .unwrap()
                .unwrap()
                .agent(agent)
                .unwrap()
                .actionable_error
                .as_deref(),
            Some(error)
        );
        std::fs::create_dir(&cwd).unwrap();
        poll_until(&mut worker, agent, |a| a.working_directory_error.is_none());
        assert_eq!(worker.state.agent(agent).unwrap().shown_status(), expected);
        assert!(worker
            .state
            .agent(agent)
            .unwrap()
            .actionable_error
            .is_none());
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn directory_recovery_keeps_an_unrelated_agent_error() {
    let (mut worker, agent, dir, cwd) =
        directory_fixture(Provider::Codex, schema::AgentStatus::Idle);
    std::fs::remove_dir(&cwd).unwrap();
    poll_until(&mut worker, agent, |a| a.working_directory_error.is_some());
    worker
        .state
        .set_agent_error(agent, Some("permission denied by provider".into()))
        .unwrap();
    worker.poll().unwrap();
    assert_eq!(
        worker
            .state
            .agent(agent)
            .unwrap()
            .actionable_error
            .as_deref(),
        Some("permission denied by provider")
    );
    std::fs::create_dir(&cwd).unwrap();
    poll_until(&mut worker, agent, |a| a.working_directory_error.is_none());
    assert_eq!(
        worker
            .state
            .agent(agent)
            .unwrap()
            .actionable_error
            .as_deref(),
        Some("permission denied by provider")
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_file_replacing_the_working_directory_also_shows_the_warning() {
    let (mut worker, agent, dir, cwd) =
        directory_fixture(Provider::Codex, schema::AgentStatus::Idle);
    std::fs::remove_dir(&cwd).unwrap();
    std::fs::write(&cwd, b"not a directory").unwrap();
    poll_until(&mut worker, agent, |a| a.actionable_error.is_some());
    assert!(worker
        .state
        .agent(agent)
        .unwrap()
        .actionable_error
        .is_some());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_later_callback_cannot_permanently_erase_a_missing_directory_warning() {
    let (mut worker, agent, dir, cwd) =
        directory_fixture(Provider::Codex, schema::AgentStatus::Idle);
    std::fs::remove_dir(&cwd).unwrap();
    poll_until(&mut worker, agent, |a| a.working_directory_error.is_some());
    let warning = worker.state.agent(agent).unwrap().actionable_error.clone();
    assert!(warning.is_some());
    // Turn starts/completions legitimately clear the transient agent error.
    worker.state.set_agent_error(agent, None).unwrap();
    poll_until(&mut worker, agent, |a| a.actionable_error.is_some());
    assert_eq!(worker.state.agent(agent).unwrap().actionable_error, warning);
    std::fs::create_dir(&cwd).unwrap();
    poll_until(&mut worker, agent, |a| a.working_directory_error.is_none());
    assert!(worker
        .state
        .agent(agent)
        .unwrap()
        .working_directory_error
        .is_none());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

struct MissingPane;

impl Transport for MissingPane {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::AgentList(_) | Method::AgentListWithDetectionContext(_) => {
                Ok(ResponseResult::AgentList { agents: Vec::new() })
            }
            Method::PaneGet(_) => Err(TransportError {
                timestamps: Default::default(),
                code: Some("pane_not_found".into()),
                message: "pane not found".into(),
                definitely_rejected: false,
            }),
            _ => Ok(ResponseResult::Ok {}),
        }
    }
}

#[test]
fn missing_directory_does_not_hide_detached_pane_recovery() {
    let (mut worker, agent, dir, cwd) =
        directory_fixture(Provider::Codex, schema::AgentStatus::Idle);
    std::fs::remove_dir(&cwd).unwrap();
    poll_until(&mut worker, agent, |a| a.working_directory_error.is_some());
    assert!(worker
        .state
        .agent(agent)
        .unwrap()
        .working_directory_error
        .is_some());
    worker.transport = Box::new(MissingPane);
    worker.poll().unwrap();
    assert!(worker
        .state
        .agent(agent)
        .unwrap()
        .runtime_identity
        .pane_id
        .is_none());
    let detached_error = worker.state.agent(agent).unwrap().actionable_error.clone();
    std::fs::create_dir(&cwd).unwrap();
    poll_until(&mut worker, agent, |a| a.working_directory_error.is_none());
    let restored_error = worker.state.agent(agent).unwrap().actionable_error.clone();
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        detached_error
            .as_deref()
            .is_some_and(|error| error.contains("bus agent resume")),
        "missing pane recovery while directory absent: {detached_error:?}"
    );
    assert!(
        restored_error
            .as_deref()
            .is_some_and(|error| error.contains("bus agent resume")),
        "missing pane recovery after directory restoration: {restored_error:?}"
    );
}

#[test]
fn missing_launch_directory_preserves_activity_in_control_state() {
    for (native, expected) in [
        (schema::AgentStatus::Working, "working"),
        (schema::AgentStatus::Idle, "idle"),
    ] {
        let (mut worker, agent, dir, cwd) = directory_fixture(Provider::Codex, native);
        std::fs::remove_dir(&cwd).unwrap();
        poll_until(&mut worker, agent, |a| a.working_directory_error.is_some());
        let response = worker.control_response_with_events(
            &crate::messaging::control::Request {
                id: "inspect-missing-directory".into(),
                method: "state".into(),
                params: json!({}),
            },
            None,
        );
        assert!(response.ok, "{response:?}");
        let state = response.result;
        let row = state["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["id"] == json!(agent))
            .unwrap();
        assert_eq!(row["status"], "unavailable");
        assert_eq!(row["activity_status"], expected);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn removed_launch_directory_refuses_admission_before_sampler() {
    for provider in [
        Provider::Codex,
        Provider::ClaudeCode,
        Provider::Cursor,
        Provider::Grok,
        Provider::Kimi,
    ] {
        let (mut worker, agent, dir, cwd) = directory_fixture(provider, schema::AgentStatus::Idle);
        let room = worker.state.agent(agent).unwrap().room_id;
        std::fs::remove_dir(&cwd).unwrap();
        let response = worker.control_response_with_events(&crate::messaging::control::Request {
            id: "removed-directory-admission".into(), method: "message.send".into(),
            params: json!({"room":room.0.to_string(),"to":[agent.0.to_string()],"text":"must not enter the queue"}),
        }, None);
        assert!(!response.ok, "{provider:?}: {response:?}");
        let error = response.error.unwrap().message;
        assert!(
            error.contains("launch directory removed") && error.contains("replace the agent"),
            "{error}"
        );
        assert!(error.contains(cwd.to_str().unwrap()), "{error}");
        assert_eq!(worker.state.requests().count(), 0);
        assert_eq!(
            worker.state.agent(agent).unwrap().shown_status(),
            RuntimeStatus::Unavailable
        );
        assert!(worker.state.agent(agent).unwrap().current_request.is_none());
        let saved = JsonStore::new(dir.join("state.json"))
            .load()
            .unwrap()
            .unwrap();
        assert_eq!(
            saved.agent(agent).unwrap().shown_status(),
            RuntimeStatus::Unavailable
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn removed_launch_directory_refuses_multi_recipient_admission_atomically() {
    let (mut worker, agent, dir, cwd) =
        directory_fixture(Provider::Codex, schema::AgentStatus::Idle);
    let room = worker.state.agent(agent).unwrap().room_id;
    let mut state = worker.state.clone();
    let healthy = state
        .create_agent(room, "healthy", Provider::Codex, dir.clone(), None)
        .unwrap();
    worker.save(state).unwrap();
    std::fs::remove_dir(&cwd).unwrap();
    let response = worker.control_response_with_events(&crate::messaging::control::Request {
        id: "removed-directory-multi".into(), method: "message.send".into(),
        params: json!({"room":room.0.to_string(),"to":[healthy.0.to_string(),agent.0.to_string()],"text":"atomic admission"}),
    }, None);
    assert!(!response.ok, "{response:?}");
    assert_eq!(worker.state.requests().count(), 0);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn removed_directory_refuses_ui_admission_atomically() {
    for queued in [false, true] {
        let (mut worker, agent, dir, cwd) =
            directory_fixture(Provider::Codex, schema::AgentStatus::Idle);
        let room = worker.state.agent(agent).unwrap().room_id;
        let mut state = worker.state.clone();
        let healthy = state
            .create_agent(room, "healthy", Provider::Codex, dir.clone(), None)
            .unwrap();
        state
            .set_draft_text(room, "preserve this UI draft")
            .unwrap();
        state.set_draft_recipients(room, [healthy, agent]).unwrap();
        let draft = state.room(room).unwrap().draft.clone();
        worker.save(state).unwrap();
        std::fs::remove_dir(&cwd).unwrap();
        let command = if queued {
            crate::messaging::coordinator::BusCommand::SubmitQueued(room, draft.clone())
        } else {
            crate::messaging::coordinator::BusCommand::Submit(room, draft.clone())
        };
        let (events, _) = std::sync::mpsc::channel();
        let result = worker.command(command, &events);
        assert!(result.is_err(), "queued={queued}: {result:?}");
        assert!(result.unwrap_err().contains("replace the agent"));
        assert_eq!(worker.state.requests().count(), 0);
        assert!(worker.state.queued_requests(healthy).is_empty());
        assert!(worker.state.queued_requests(agent).is_empty());
        assert_eq!(worker.state.room(room).unwrap().draft, draft);
        let durable = JsonStore::new(dir.join("state.json"))
            .load()
            .unwrap()
            .unwrap();
        assert_eq!(durable.requests().count(), 0);
        assert_eq!(durable.room(room).unwrap().draft, draft);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

fn directory_running_request(worker: &mut Worker, room: RoomId, agent: AgentId) -> RequestId {
    let request = queue(worker, room, agent, "already running");
    let mut state = worker.state.clone();
    state.begin_submission(request, "launch", 0).unwrap();
    state
        .record_submission(
            request,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("session".into()),
                provider_turn_id: None,
            },
        )
        .unwrap();
    state
        .observe_status(agent, RuntimeStatus::Working, io::now_ms())
        .unwrap();
    worker.save(state).unwrap();
    request
}

fn assert_directory_closed(worker: &Worker, agent: AgentId, requests: &[RequestId]) {
    for request in requests {
        let request = worker.state.request(*request).unwrap();
        assert_eq!(request.delivery_stage(), "abandoned", "{request:?}");
        assert_eq!(
            request.failure_reason.as_deref(),
            Some("launch_directory_removed")
        );
        assert!(request.reply().is_none());
    }
    assert!(worker.state.agent(agent).unwrap().current_request.is_none());
    assert!(worker.state.queued_requests(agent).is_empty());
    assert_eq!(
        worker.state.agent(agent).unwrap().shown_status(),
        RuntimeStatus::Unavailable
    );
}

#[test]
fn removed_launch_directory_hook_failure_closes_current_and_queued_work() {
    let (mut worker, agent, dir, cwd) =
        directory_fixture(Provider::Codex, schema::AgentStatus::Working);
    let room = worker.state.agent(agent).unwrap().room_id;
    let running = directory_running_request(&mut worker, room, agent);
    let queued = queue(&mut worker, room, agent, "queued behind the broken hook");
    std::fs::remove_dir(&cwd).unwrap();
    let error = worker
        .agent_error(
            agent,
            "Hook failed: No such file or directory (os error 2)".into(),
        )
        .unwrap_err();
    assert!(error.contains("launch directory removed"), "{error}");
    assert_directory_closed(&worker, agent, &[running, queued]);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn removed_launch_directory_sampler_closes_work_without_overwriting_captured_reply() {
    let (mut worker, agent, dir, cwd) =
        directory_fixture(Provider::Codex, schema::AgentStatus::Working);
    let room = worker.state.agent(agent).unwrap().room_id;
    let answered = directory_running_request(&mut worker, room, agent);
    let mut state = worker.state.clone();
    let message = state.request(answered).unwrap().prompt.id;
    state
        .answer_message(message, agent, "captured answer".into(), io::now_ms())
        .unwrap();
    let captured = state.request(answered).unwrap().clone();
    worker.save(state).unwrap();
    let queued = queue(&mut worker, room, agent, "never run");
    std::fs::remove_dir(&cwd).unwrap();
    poll_until(&mut worker, agent, |a| a.working_directory_error.is_some());
    assert_directory_closed(&worker, agent, &[queued]);
    let kept = worker.state.request(answered).unwrap();
    assert_eq!(kept.completed_reply, captured.completed_reply);
    assert_eq!(kept.completed_at_ms, captured.completed_at_ms);
    assert_eq!(kept.failure_reason, captured.failure_reason);
    assert_eq!(kept.delivery_stage(), "replied");
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn removed_launch_directory_queued_delivery_is_abandoned_before_native_submit() {
    let (mut worker, agent, dir, cwd) =
        directory_fixture(Provider::Codex, schema::AgentStatus::Idle);
    let room = worker.state.agent(agent).unwrap().room_id;
    let request = queue(&mut worker, room, agent, "queued by the UI");
    std::fs::remove_dir(&cwd).unwrap();
    worker.submit_ready().unwrap();
    assert_directory_closed(&worker, agent, &[request]);
    assert!(
        worker
            .state
            .request(request)
            .unwrap()
            .expected_launch_id
            .is_none(),
        "native submission must not begin"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

struct RemovedDuringSubmission(PathBuf);
impl Transport for RemovedDuringSubmission {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        panic!("unexpected synchronous request: {method:?}")
    }
    fn submit(&mut self, _: Method) -> crate::messaging::native::NativeCall {
        std::fs::remove_dir(&self.0).unwrap();
        crate::messaging::native::NativeCall::Complete(Box::new(Err(TransportError {
            code: Some("session_hook_missing".into()),
            message: "Hook failed: No such file or directory (os error 2)".into(),
            definitely_rejected: true,
            timestamps: Default::default(),
        })))
    }
}

#[test]
fn removed_launch_directory_during_native_failure_closes_submission() {
    let (mut worker, agent, dir, cwd) =
        directory_fixture(Provider::Codex, schema::AgentStatus::Idle);
    let room = worker.state.agent(agent).unwrap().room_id;
    let request = queue(&mut worker, room, agent, "removed after admission");
    worker.transport = Box::new(RemovedDuringSubmission(cwd));
    worker.submit_ready().unwrap();
    assert_directory_closed(&worker, agent, &[request]);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn removed_launch_directory_turn_tracking_closes_current_without_waiting_for_hooks() {
    let (mut worker, agent, dir, cwd) =
        directory_fixture(Provider::Codex, schema::AgentStatus::Working);
    let room = worker.state.agent(agent).unwrap().room_id;
    let running = directory_running_request(&mut worker, room, agent);
    std::fs::remove_dir(&cwd).unwrap();
    worker.settle_requests().unwrap();
    assert_directory_closed(&worker, agent, &[running]);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn ui_admission_keeps_the_fresh_directory_recovery() {
    for queued in [false, true] {
        let (mut worker, agent, dir, cwd) =
            directory_fixture(Provider::Codex, schema::AgentStatus::Idle);
        let room = worker.state.agent(agent).unwrap().room_id;
        std::fs::remove_dir(&cwd).unwrap();
        worker.check_launch_directory(agent).unwrap().unwrap();
        std::fs::create_dir(&cwd).unwrap();
        let draft = Draft {
            text: "healthy UI admission".into(),
            files: Vec::new(),
            recipient_ids: [agent].into(),
        };
        let command = if queued {
            BusCommand::SubmitQueued(room, draft)
        } else {
            BusCommand::Submit(room, draft)
        };
        let (events, _) = std::sync::mpsc::channel();
        worker.command(command, &events).unwrap();
        assert_eq!(worker.state.requests().count(), 1);
        assert_eq!(worker.state.queued_requests(agent).len(), 1);
        assert!(
            worker
                .state
                .agent(agent)
                .unwrap()
                .working_directory_error
                .is_none(),
            "queued={queued}"
        );
        assert_eq!(
            worker.state.agent(agent).unwrap().shown_status(),
            RuntimeStatus::Idle
        );
        let durable = JsonStore::new(dir.join("state.json"))
            .load()
            .unwrap()
            .unwrap();
        assert!(durable
            .agent(agent)
            .unwrap()
            .working_directory_error
            .is_none());
        assert_eq!(
            durable.queued_requests(agent),
            worker.state.queued_requests(agent)
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
