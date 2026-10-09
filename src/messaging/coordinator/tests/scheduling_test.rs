use super::*;

struct PollingNative {
    commands: mpsc::Sender<(u64, BusCommand)>,
    submitted: mpsc::Sender<()>,
    info: schema::AgentInfo,
    message: PromptId,
    shutdown: bool,
}

impl Transport for PollingNative {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::AgentList(_) => {
                // Model `send --async` polling during every status round trip.
                let command = if self.shutdown {
                    BusCommand::Shutdown
                } else {
                    let (reply, _) = mpsc::sync_channel(1);
                    BusCommand::Dev(crate::messaging::control::server::DevCall {
                        request: crate::messaging::control::Request {
                            id: format!("status-{}", io::now_ns()),
                            method: "message.status".into(),
                            params: json!({"message": self.message.0.to_string()}),
                        },
                        reply,
                    })
                };
                self.commands.send((1, command)).unwrap();
                Ok(ResponseResult::AgentList {
                    agents: vec![self.info.clone()],
                })
            }
            Method::AgentPromptIfIdle(_) | Method::AgentPromptIfUnbound(_) => {
                self.submitted.send(()).unwrap();
                Ok(ResponseResult::AgentPrompted {
                    agent: self.info.clone(),
                })
            }
            _ => panic!("unexpected terminal operation"),
        }
    }
}

#[test]
fn status_polling_during_every_tick_cannot_starve_idle_first_submission() {
    for (provider, fresh) in [
        (Provider::Codex, false),
        (Provider::Codex, true),
        (Provider::ClaudeCode, false),
    ] {
        let (mut worker, agent, room, dir, _) = fixture(provider, vec![]);
        worker.dev_enabled = true;
        let mut info = native_info("pane", launch::provider_kind(provider), "session");
        info.interactive_ready = true;
        if fresh {
            let mut state = worker.state.clone();
            let mut identity = state.agent(agent).unwrap().runtime_identity.clone();
            identity.session_id = None;
            state.set_agent_runtime_identity(agent, identity).unwrap();
            worker.save(state).unwrap();
            info.agent_session = None;
        }
        let request = queue(&mut worker, room, agent, "start work");
        let message = worker.state.request(request).unwrap().prompt.id;
        let (commands, receiver) = mpsc::channel();
        let (submitted, observed) = mpsc::channel();
        worker.transport = Box::new(PollingNative {
            commands: commands.clone(),
            submitted,
            info,
            message,
            shutdown: false,
        });
        let snapshot = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
        let (events, _) = mpsc::channel();
        let thread = std::thread::spawn(move || worker.run(receiver, events, snapshot));
        let result = observed.recv_timeout(Duration::from_secs(2));
        commands.send((2, BusCommand::Shutdown)).unwrap();
        thread.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        assert!(
            result.is_ok(),
            "{provider:?} fresh={fresh}: read-only polls prevented every submit"
        );
    }
}

#[test]
fn a_shutdown_received_during_poll_still_prevents_delivery() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let request = queue(&mut worker, room, agent, "must not send");
    let message = worker.state.request(request).unwrap().prompt.id;
    let (commands, receiver) = mpsc::channel();
    let (submitted, observed) = mpsc::channel();
    let mut info = native_info("pane", "codex", "session");
    info.interactive_ready = true;
    worker.transport = Box::new(PollingNative {
        commands,
        submitted,
        info,
        message,
        shutdown: true,
    });
    let snapshot = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
    let (events, _) = mpsc::channel();
    let thread = std::thread::spawn(move || worker.run(receiver, events, snapshot));
    thread.join().unwrap();
    assert!(observed.try_recv().is_err());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_stalled_queued_message_is_closed_and_never_fires_after_readiness_returns() {
    let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
    worker.dev_enabled = true;
    let request = stalled_queue(&mut worker, agent, room);
    let now = io::now_ms();
    let message = worker.state.request(request).unwrap().prompt.id;
    assert_eq!(
        worker.dev_message_at(message, now).unwrap()["requests"][0]["stage"],
        "stalled"
    );
    worker.submit_ready().unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Abandoned
    );
    assert!(worker.state.queued_requests(agent).is_empty());
    let response = worker.dev_message_at(message, now).unwrap();
    assert_eq!(response["complete"], true);
    assert_eq!(response["requests"][0]["reason"], "not_submitted");
    assert!(!calls
        .lock()
        .unwrap()
        .iter()
        .any(|method| method.starts_with("agent.prompt")));
    drop(worker);
    let mut reopened = reopen_saved(&dir);
    reopened
        .state
        .observe_status(agent, RuntimeStatus::Idle, now + 1)
        .unwrap();
    reopened.submit_ready().unwrap();
    assert_eq!(
        reopened.state.request(request).unwrap().phase,
        RequestPhase::Abandoned
    );
    drop(reopened);
    std::fs::remove_dir_all(dir).unwrap();
}

fn stalled_queue(worker: &mut Worker, agent: AgentId, room: RoomId) -> RequestId {
    let request = queue(worker, room, agent, "old work");
    let now = io::now_ms();
    let mut saved = serde_json::to_value(&worker.state).unwrap();
    saved["requests"][request.0.to_string()]["prompt"]["submitted_at_ms"] =
        (now - crate::messaging::model::QUEUED_STALL_MS).into();
    let mut state = serde_json::from_value::<BusState>(saved).unwrap();
    state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
    state
        .observe_status(agent, RuntimeStatus::Idle, now)
        .unwrap();
    worker.save(state).unwrap();
    request
}

#[test]
fn reporting_a_queued_stall_closes_it_durably_before_the_sender_stops_waiting() {
    let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
    let request = stalled_queue(&mut worker, agent, room);
    let message = worker.state.request(request).unwrap().prompt.id;
    let response = worker.dev_message(message).unwrap();
    assert_eq!(response["complete"], true);
    assert_eq!(response["requests"][0]["stage"], "abandoned");
    assert_eq!(response["requests"][0]["reason"], "not_submitted");
    let saved = worker.store.load().unwrap().unwrap();
    assert_eq!(
        saved.request(request).unwrap().phase,
        RequestPhase::Abandoned
    );
    assert!(saved.queued_requests(agent).is_empty());
    assert!(calls.lock().unwrap().is_empty());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn opening_saved_stalled_work_closes_it_before_resetting_readiness() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let request = stalled_queue(&mut worker, agent, room);
    drop(worker);
    let mut reopened = reopen_saved(&dir);
    reopened
        .state
        .observe_status(agent, RuntimeStatus::Idle, io::now_ms())
        .unwrap();
    reopened.submit_ready().unwrap();
    assert_eq!(
        reopened.state.request(request).unwrap().phase,
        RequestPhase::Abandoned
    );
    drop(reopened);
    std::fs::remove_dir_all(dir).unwrap();
}
