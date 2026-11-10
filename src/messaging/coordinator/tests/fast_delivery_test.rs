use super::*;

struct WakeNative {
    info: schema::AgentInfo,
    polled: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
    submitted: mpsc::Sender<()>,
    first: bool,
}

impl Transport for WakeNative {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::AgentListWithDetectionContext(_) => {
                if self.first {
                    self.first = false;
                    self.polled.send(()).unwrap();
                    self.release.recv().unwrap();
                }
                Ok(ResponseResult::AgentList {
                    agents: vec![self.info.clone()],
                })
            }
            Method::AgentPromptIfIdleAtGeneration(_) => {
                self.submitted.send(()).unwrap();
                Ok(ResponseResult::AgentPrompted {
                    agent: self.info.clone(),
                    timestamps: Default::default(),
                })
            }
            other => panic!("unexpected native operation: {other:?}"),
        }
    }
}

#[test]
fn accepted_work_wakes_delivery_before_the_next_full_poll() {
    let _providers = provider_binaries();
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    worker.dev_enabled = true;
    let mut info = native_info("pane", "codex", "session");
    info.interactive_ready = true;
    let (polled, observed_poll) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let (submitted, observed_submit) = mpsc::channel();
    worker.transport = Box::new(WakeNative {
        info,
        polled,
        release: released,
        submitted,
        first: true,
    });
    let (commands, receiver) = mpsc::channel();
    let snapshots = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
    let (events, _) = mpsc::channel();
    let thread = std::thread::spawn(move || worker.run(receiver, events, snapshots));
    observed_poll.recv_timeout(Duration::from_secs(2)).unwrap();
    let (reply, result) = mpsc::sync_channel(1);
    commands.send((1, BusCommand::Control(crate::messaging::control::server::ControlCall {
        request: crate::messaging::control::Request {
            id: "fast-wake".into(), method: "message.send".into(),
            params: json!({"room":room.0.to_string(),"to":[agent.0.to_string()],"text":"start now"}),
        }, reply, deadline: None, _poll_permit: None, form_generation: None,
    }))).unwrap();
    release.send(()).unwrap();
    assert!(result.recv_timeout(Duration::from_secs(2)).unwrap().ok);
    let delivered = observed_submit.recv_timeout(Duration::from_millis(200));
    commands.send((2, BusCommand::Shutdown)).unwrap();
    thread.join().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        delivered.is_ok(),
        "an accepted eligible message slept until the next 500 ms full poll"
    );
}

struct TimestampNative {
    path: PathBuf,
    request: RequestId,
}
impl Transport for TimestampNative {
    fn request(&mut self, _: Method) -> Result<ResponseResult, TransportError> {
        let saved = JsonStore::new(self.path.clone()).load().unwrap().unwrap();
        assert!(
            saved
                .request(self.request)
                .unwrap()
                .latency
                .native_requested_at_ms
                .is_some(),
            "the native-request boundary must be durable before native I/O"
        );
        Ok(ResponseResult::AgentPrompted {
            agent: native_info("pane", "codex", "session"),
            timestamps: Default::default(),
        })
    }
}

#[test]
fn native_request_is_stamped_before_crossing_the_native_boundary() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let request = queue(&mut worker, room, agent, "stamp before send");
    worker.transport = Box::new(TimestampNative {
        path: dir.join("state.json"),
        request,
    });
    worker.submit_ready().unwrap();
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

type DeferredSubmissions = Vec<(Method, mpsc::Sender<crate::messaging::native::NativeResult>)>;

#[derive(Clone)]
struct DeferredNative(Arc<Mutex<DeferredSubmissions>>);
impl Transport for DeferredNative {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        panic!("unexpected synchronous fast-path request: {method:?}")
    }
    fn submit(&mut self, method: Method) -> crate::messaging::native::NativeCall {
        let (tx, rx) = mpsc::channel();
        self.0.lock().unwrap().push((method, tx));
        crate::messaging::native::NativeCall::Pending(rx)
    }
}

fn confirmed() -> Result<ResponseResult, TransportError> {
    Ok(ResponseResult::AgentPrompted {
        agent: native_info("pane", "codex", "session"),
        timestamps: Default::default(),
    })
}

#[test]
fn a_slow_recipient_does_not_hold_another_recipient_or_duplicate_its_own_write() {
    let (mut worker, a, room, dir, _) = fixture(Provider::Codex, vec![]);
    let mut state = worker.state.clone();
    let b = state
        .create_agent(room, "second", Provider::Codex, dir.clone(), None)
        .unwrap();
    let mut identity = state.agent(a).unwrap().runtime_identity.clone();
    identity.launch_id = Some("launch-b".into());
    identity.terminal_id = Some("terminal-b".into());
    identity.pane_id = Some("pane-b".into());
    state.set_agent_runtime_identity(b, identity).unwrap();
    state.confirm_hook_setup(b).unwrap();
    state.observe_status(b, RuntimeStatus::Idle, 1).unwrap();
    worker.save(state).unwrap();
    callbacks::initialize(
        &dir.join("callbacks/launch-b"),
        &callbacks::Manifest {
            routing_key: callbacks::RoutingKey(b.0),
            provider: Provider::Codex,
            launch_id: "launch-b".into(),
        },
    )
    .unwrap();
    let first = queue(&mut worker, room, a, "slow composer");
    let second = queue(&mut worker, room, b, "fast composer");
    let native = DeferredNative(Default::default());
    worker.transport = Box::new(native.clone());
    worker.submit_ready().unwrap();
    assert_eq!(
        native.0.lock().unwrap().len(),
        2,
        "both owned writes start before either receipt"
    );
    worker.submit_ready().unwrap();
    assert_eq!(
        native.0.lock().unwrap().len(),
        2,
        "an in-flight recipient cannot be submitted twice"
    );
    native.0.lock().unwrap()[1].1.send(confirmed()).unwrap();
    worker.finish_native_submissions(false);
    assert_eq!(
        worker.state.request(first).unwrap().phase,
        RequestPhase::Submitting
    );
    assert_eq!(
        worker.state.request(second).unwrap().phase,
        RequestPhase::Active
    );
    native.0.lock().unwrap()[0].1.send(confirmed()).unwrap();
    worker.finish_native_submissions(false);
    assert!(worker.native_submissions.is_empty());
    assert_eq!(
        worker.state.request(first).unwrap().phase,
        RequestPhase::Active
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn fast_native_admission_keeps_provider_identity_and_busy_routing_guards() {
    for provider in [
        Provider::Codex,
        Provider::ClaudeCode,
        Provider::ClaudeDs,
        Provider::Cursor,
        Provider::Kimi,
        Provider::Grok,
    ] {
        let (mut worker, agent, room, dir, _) = fixture(provider, vec![]);
        worker
            .state
            .observe_status(agent, RuntimeStatus::Working, 2)
            .unwrap();
        let request = queue(&mut worker, room, agent, "follow up");
        let native = DeferredNative(Default::default());
        worker.transport = Box::new(native.clone());
        worker.submit_ready().unwrap();
        if provider == Provider::Grok {
            assert!(
                native.0.lock().unwrap().is_empty(),
                "Grok keeps its busy hold"
            );
            assert!(worker
                .state
                .request(request)
                .unwrap()
                .latency
                .eligible_at_ms
                .is_none());
        } else {
            let calls = native.0.lock().unwrap();
            let Method::AgentPromptIfIdleAtGeneration(params) = &calls[0].0 else {
                panic!("guarded submission expected")
            };
            assert!(params.steer);
            assert_eq!(params.expected_terminal_id, "terminal");
            assert_eq!(params.expected_pane_id, "pane");
            assert_eq!(params.expected_session_id, "session");
            assert_eq!(params.expected_agent, launch::provider_kind(provider));
            assert_eq!(params.expected_generation, Some(worker.serving_generation));
            calls[0].1.send(confirmed()).unwrap();
            drop(calls);
            worker.finish_native_submissions(false);
        }
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn late_native_receipt_does_not_bind_to_a_replaced_terminal() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let request = queue(&mut worker, room, agent, "old owner");
    let native = DeferredNative(Default::default());
    worker.transport = Box::new(native.clone());
    worker.submit_ready().unwrap();
    let mut state = worker.state.clone();
    let mut identity = state.agent(agent).unwrap().runtime_identity.clone();
    identity.terminal_id = Some("replacement".into());
    state.set_agent_runtime_identity(agent, identity).unwrap();
    worker.save(state).unwrap();
    native.0.lock().unwrap()[0].1.send(confirmed()).unwrap();
    worker.finish_native_submissions(false);
    assert_ne!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Active
    );
    assert_eq!(
        worker
            .state
            .agent(agent)
            .unwrap()
            .runtime_identity
            .terminal_id
            .as_deref(),
        Some("replacement")
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn reopening_an_inflight_submission_preserves_timestamps_without_replaying() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let request = queue(&mut worker, room, agent, "pending across restart");
    let native = DeferredNative(Default::default());
    worker.transport = Box::new(native.clone());
    worker.submit_ready().unwrap();
    let before = worker.state.request(request).unwrap().latency.clone();
    assert!(before.eligible_at_ms.is_some());
    assert!(before.native_requested_at_ms.is_some());
    drop(worker);
    let mut reopened = Worker::open(dir.clone(), Box::new(native.clone())).unwrap();
    reopened.submit_ready().unwrap();
    assert_eq!(native.0.lock().unwrap().len(), 1);
    let after = &reopened.state.request(request).unwrap().latency;
    assert_eq!(after.accepted_at_ms, before.accepted_at_ms);
    assert_eq!(after.eligible_at_ms, before.eligible_at_ms);
    assert_eq!(after.picked_at_ms, before.picked_at_ms);
    assert_eq!(after.native_requested_at_ms, before.native_requested_at_ms);
    assert!(after.native.native_confirmed_at_ms.is_none());
    assert_eq!(
        reopened.state.request(request).unwrap().phase,
        RequestPhase::Submitting
    );
    drop(reopened);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn shutdown_drains_the_native_write_before_releasing_the_lease() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let request = queue(&mut worker, room, agent, "owned through shutdown");
    let native = DeferredNative(Default::default());
    worker.transport = Box::new(native.clone());
    worker.submit_ready().unwrap();
    let (done, observed) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        worker.finish_native_submissions(true);
        done.send(()).unwrap();
        worker
    });
    assert!(observed.recv_timeout(Duration::from_millis(25)).is_err());
    assert!(Worker::open(dir.clone(), Box::new(native.clone())).is_err());
    native.0.lock().unwrap()[0].1.send(confirmed()).unwrap();
    observed.recv_timeout(Duration::from_secs(2)).unwrap();
    let worker = thread.join().unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Active
    );
    assert!(worker.native_submissions.is_empty());
    drop(worker);
    let reopened = Worker::open(dir.clone(), Box::new(native)).unwrap();
    assert_eq!(
        reopened.state.request(request).unwrap().phase,
        RequestPhase::Active
    );
    drop(reopened);
    std::fs::remove_dir_all(dir).unwrap();
}

struct RevocationNative {
    deferred: DeferredNative,
    mutation: mpsc::Sender<()>,
}
impl Transport for RevocationNative {
    fn request(&mut self, _: Method) -> Result<ResponseResult, TransportError> {
        self.mutation.send(()).unwrap();
        Err(TransportError {
            code: Some("test_mutation_rejected".into()),
            message: "No terminal exists in this fixture".into(),
            definitely_rejected: true,
            timestamps: Default::default(),
        })
    }
    fn submit(&mut self, method: Method) -> crate::messaging::native::NativeCall {
        self.deferred.submit(method)
    }
}

#[test]
fn an_interrupt_cannot_overtake_a_pending_native_write() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    worker.dev_enabled = true;
    worker
        .state
        .observe_status(agent, RuntimeStatus::Working, 2)
        .unwrap();
    queue(&mut worker, room, agent, "owned write before stop");
    let deferred = DeferredNative(Default::default());
    let (mutation, observed_mutation) = mpsc::channel();
    worker.transport = Box::new(RevocationNative {
        deferred: deferred.clone(),
        mutation,
    });
    worker.submit_ready().unwrap();
    let (reply, response) = mpsc::sync_channel(1);
    let snapshots = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
    let (events, _) = mpsc::channel();
    let (commands, received) = mpsc::channel();
    commands
        .send((
            1,
            BusCommand::Control(crate::messaging::control::server::ControlCall {
                request: crate::messaging::control::Request {
                    id: "ordered-stop".into(),
                    method: "agent.interrupt".into(),
                    params: json!({"agent":agent.0.to_string()}),
                },
                reply,
                deadline: None,
                _poll_permit: None,
                form_generation: None,
            }),
        ))
        .unwrap();
    let thread = std::thread::spawn(move || worker.run(received, events, snapshots));
    let overtook = observed_mutation
        .recv_timeout(Duration::from_millis(200))
        .is_ok();
    deferred.0.lock().unwrap()[0].1.send(confirmed()).unwrap();
    response.recv_timeout(Duration::from_secs(2)).unwrap();
    let eventually_sent = overtook
        || observed_mutation
            .recv_timeout(Duration::from_secs(2))
            .is_ok();
    commands.send((2, BusCommand::Shutdown)).unwrap();
    thread.join().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        eventually_sent,
        "the stop must eventually reach native admission"
    );
    assert!(
        !overtook,
        "the stop RPC overtook a native write that could still land afterward"
    );
}
