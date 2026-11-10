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

struct ReadinessWhilePending {
    deferred: DeferredNative,
    infos: Vec<schema::AgentInfo>,
    ready_recipient: mpsc::Sender<()>,
}

impl Transport for ReadinessWhilePending {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::AgentListWithDetectionContext(_) => Ok(ResponseResult::AgentList {
                agents: self.infos.clone(),
            }),
            other => panic!("unexpected readiness operation: {other:?}"),
        }
    }

    fn submit(&mut self, method: Method) -> crate::messaging::native::NativeCall {
        let newly_ready = matches!(&method,
            Method::AgentPromptIfIdleAtGeneration(params) if params.target == "pane-b");
        let call = self.deferred.submit(method);
        if newly_ready {
            self.ready_recipient.send(()).unwrap();
        }
        call
    }
}

#[test]
fn a_pending_write_does_not_hold_another_recipients_readiness_refresh() {
    let (mut worker, a, room, dir, _) = fixture(Provider::Codex, vec![]);
    worker.dev_enabled = true;
    let mut state = worker.state.clone();
    let b = state
        .create_agent(room, "newly-ready", Provider::Codex, dir.clone(), None)
        .unwrap();
    let mut identity = state.agent(a).unwrap().runtime_identity.clone();
    identity.launch_id = Some("launch-b".into());
    identity.terminal_id = Some("terminal-b".into());
    identity.pane_id = Some("pane-b".into());
    identity.session_id = Some("session-b".into());
    state.set_agent_runtime_identity(b, identity).unwrap();
    state.confirm_hook_setup(b).unwrap();
    state.observe_status(b, RuntimeStatus::Blocked, 1).unwrap();
    state.observe_dialog(b, true, 1).unwrap();
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
    queue(&mut worker, room, a, "slow native write");
    queue(&mut worker, room, b, "dialog has now closed");
    let mut first = native_info("pane", "codex", "session");
    first.interactive_ready = true;
    let mut second = native_info("pane-b", "codex", "session-b");
    second.terminal_id = "terminal-b".into();
    second.name = Some(format!("bus-r{}-a{}", room.0, b.0));
    second.interactive_ready = true;
    let deferred = DeferredNative(Default::default());
    let (ready_recipient, submitted) = mpsc::channel();
    worker.transport = Box::new(ReadinessWhilePending {
        deferred: deferred.clone(),
        infos: vec![first, second.clone()],
        ready_recipient,
    });
    worker.submit_ready().unwrap();
    assert_eq!(deferred.0.lock().unwrap().len(), 1);
    let snapshots = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
    let (events, _) = mpsc::channel();
    let (commands, received) = mpsc::channel();
    let (prepared, fixture_ready) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let _providers = provider_binaries();
        prepared.send(()).unwrap();
        worker.run(received, events, snapshots);
    });
    fixture_ready.recv_timeout(Duration::from_secs(5)).unwrap();
    // Prove ordering against the held receipt, without charging fixture I/O or
    // host contention to a synthetic subsecond performance threshold.
    let independent = submitted.recv_timeout(Duration::from_secs(2)).is_ok();
    deferred.0.lock().unwrap()[0].1.send(confirmed()).unwrap();
    if !independent {
        submitted.recv_timeout(Duration::from_secs(2)).unwrap();
    }
    deferred.0.lock().unwrap()[1]
        .1
        .send(Ok(ResponseResult::AgentPrompted {
            agent: second,
            timestamps: Default::default(),
        }))
        .unwrap();
    commands.send((1, BusCommand::Shutdown)).unwrap();
    thread.join().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        independent,
        "a pending native write blocked another recipient's readiness refresh"
    );
}

#[test]
fn a_pending_write_is_reconciled_before_generation_settlement() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    worker.serving_generation = 1;
    let request = queue(
        &mut worker,
        room,
        agent,
        "old generation write is still running",
    );
    let native = DeferredNative(Default::default());
    worker.transport = Box::new(native.clone());
    worker.submit_ready().unwrap();
    worker.serving_generation = 2;
    worker.settle_requests().unwrap();
    let during = worker.state.request(request).unwrap().phase;
    let mut info = native_info("pane", "codex", "session");
    info.generation = 1;
    native.0.lock().unwrap()[0]
        .1
        .send(Ok(ResponseResult::AgentPrompted {
            agent: info,
            timestamps: Default::default(),
        }))
        .unwrap();
    worker.finish_native_submissions(false);
    worker.settle_requests().unwrap();
    let reconciled = worker.state.request(request).unwrap().settled();
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(
        during,
        RequestPhase::Submitting,
        "settlement overtook the native receipt"
    );
    assert!(
        reconciled,
        "generation reconciliation must resume after the receipt"
    );
}

#[test]
fn pending_writes_defer_expiry_without_holding_other_unconfirmed_requests() {
    let (mut worker, a, room, dir, _) = fixture(Provider::Codex, vec![]);
    let pending = queue(&mut worker, room, a, "native call still running");
    let native = DeferredNative(Default::default());
    worker.transport = Box::new(native.clone());
    worker.submit_ready().unwrap();
    let mut state = worker.state.clone();
    let b = state
        .create_agent(room, "unknown-outcome", Provider::Codex, dir.clone(), None)
        .unwrap();
    let mut identity = state.agent(a).unwrap().runtime_identity.clone();
    identity.launch_id = Some("launch-b".into());
    identity.terminal_id = Some("terminal-b".into());
    identity.pane_id = Some("pane-b".into());
    identity.session_id = Some("session-b".into());
    state.set_agent_runtime_identity(b, identity).unwrap();
    state.confirm_hook_setup(b).unwrap();
    state
        .observe_status(b, RuntimeStatus::Idle, io::now_ms())
        .unwrap();
    worker.save(state).unwrap();
    let unknown = queue(&mut worker, room, b, "completed call with unknown outcome");
    let mut state = worker.state.clone();
    state.begin_submission(unknown, "launch-b", 1).unwrap();
    state
        .record_submission(
            unknown,
            SubmissionOutcome::Uncertain {
                message: "lost acknowledgement".into(),
            },
        )
        .unwrap();
    worker.save(state).unwrap();
    let deadline = io::now_ms() + crate::messaging::model::DELIVERED_STALL_MS + 1_000;
    worker.expire_unconfirmed_submissions(deadline).unwrap();
    let during = worker.state.request(pending).unwrap().phase;
    let other_expired = worker
        .state
        .request(unknown)
        .unwrap()
        .failure_reason
        .as_deref()
        == Some("submission_unconfirmed");
    native.0.lock().unwrap()[0]
        .1
        .send(Err(TransportError {
            code: None,
            message: "late transport loss".into(),
            definitely_rejected: false,
            timestamps: Default::default(),
        }))
        .unwrap();
    worker.finish_native_submissions(false);
    worker
        .expire_unconfirmed_submissions(deadline + 1_000)
        .unwrap();
    let resumed = worker
        .state
        .request(pending)
        .unwrap()
        .failure_reason
        .as_deref()
        == Some("submission_unconfirmed");
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(
        during,
        RequestPhase::Submitting,
        "expiry overtook an unfinished native call"
    );
    assert!(
        other_expired,
        "another agent's unknown outcome must still expire"
    );
    assert!(resumed, "expiry must resume once native I/O has returned");
}

#[test]
fn a_failed_pre_send_timestamp_never_strands_an_unsent_submission() {
    use crate::messaging::storage::state_store::SaveStage;
    // Some skip count lets the prompt save commit and fails the next
    // checkpointed write: the native_requested journal entry.
    for skips in 0..4 {
        let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
        let request = queue(&mut worker, room, agent, "stamp fails");
        let native = DeferredNative(Default::default());
        worker.transport = Box::new(native.clone());
        worker.store.fail_once_at_after(SaveStage::TempWrite, skips);
        let _ = worker.submit_ready();
        let sent = !native.0.lock().unwrap().is_empty();
        let durable = JsonStore::new(dir.join("state.json"))
            .load()
            .unwrap()
            .unwrap()
            .request(request)
            .unwrap()
            .phase;
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
        assert!(
            sent || durable != RequestPhase::Submitting,
            "skips={skips}: request is durably Submitting but was never sent; \
             expiry will fail it as submission_unconfirmed"
        );
    }
}

#[test]
fn a_failed_pre_send_timestamp_never_strands_an_unsent_steering_member() {
    use crate::messaging::storage::state_store::SaveStage;
    for skips in 0..4 {
        let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![confirmed()]);
        queue(&mut worker, room, agent, "owned lead");
        worker.submit_ready().unwrap();
        record(
            &dir,
            Provider::Codex,
            json!({"hook_event_name":"UserPromptSubmit", "session_id":"session", "turn_id":"lead-turn", "prompt":"owned lead"}),
        );
        worker
            .consume_callbacks(agent, &dir.join("callbacks/launch"))
            .unwrap();
        worker
            .state
            .observe_status(agent, RuntimeStatus::Working, io::now_ms())
            .unwrap();
        let request = queue(&mut worker, room, agent, "steer stamp fails");
        let native = DeferredNative(Default::default());
        worker.transport = Box::new(native.clone());
        worker.store.fail_once_at_after(SaveStage::TempWrite, skips);
        let _ = worker.submit_ready();
        let sent = !native.0.lock().unwrap().is_empty();
        let durable = JsonStore::new(dir.join("state.json"))
            .load()
            .unwrap()
            .unwrap()
            .request(request)
            .unwrap()
            .phase;
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
        assert!(
            sent || durable != RequestPhase::Submitting,
            "skips={skips}: steering member is durably Submitting but was never sent"
        );
    }
}

#[test]
fn a_claude_turn_starting_between_ticks_keeps_unbound_steering_admission() {
    for provider in [Provider::ClaudeCode, Provider::ClaudeDs] {
        let (mut worker, agent, room, dir, _) = fixture(provider, vec![]);
        worker
            .consume_callbacks(agent, &dir.join("callbacks/launch"))
            .unwrap();
        assert!(!worker.own_turns.contains_key(&agent));
        record(
            &dir,
            provider,
            json!({"hook_event_name":"UserPromptSubmit", "session_id":"session", "prompt_id":"background-turn", "prompt":"autonomous background notification"}),
        );
        queue(&mut worker, room, agent, "developer correction");
        let native = DeferredNative(Default::default());
        worker.transport = Box::new(native.clone());
        worker.submit_eligible_while(|| true).unwrap();
        let steer = match &native.0.lock().unwrap()[0].0 {
            Method::AgentPromptIfIdleAtGeneration(params) => params.steer,
            other => panic!("unexpected admission {other:?}"),
        };
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
        assert!(
            steer,
            "the between-tick autonomous turn was submitted as idle instead of steering"
        );
    }
}

#[test]
fn a_queue_only_claude_message_holds_for_a_between_tick_autonomous_turn() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    record(
        &dir,
        Provider::ClaudeCode,
        json!({"hook_event_name":"UserPromptSubmit", "session_id":"session", "prompt_id":"background-turn", "prompt":"autonomous background notification"}),
    );
    let mut state = worker.state.clone();
    let request = state
        .submit_message_with(
            room,
            crate::messaging::model::Draft {
                text: "independent turn".into(),
                files: vec![],
                recipient_ids: [agent].into(),
            },
            crate::messaging::model::Author::Developer,
            io::now_ms(),
            true,
        )
        .unwrap()[0];
    worker.save(state).unwrap();
    let native = DeferredNative(Default::default());
    worker.transport = Box::new(native.clone());
    worker.submit_eligible_while(|| true).unwrap();
    let submitted = !native.0.lock().unwrap().is_empty();
    let phase = worker.state.request(request).unwrap().phase;
    record(
        &dir,
        Provider::ClaudeCode,
        json!({"hook_event_name":"Stop", "session_id":"session", "prompt_id":"background-turn", "last_assistant_message":"background finished"}),
    );
    worker.submit_eligible_while(|| true).unwrap();
    let after_stop = native.0.lock().unwrap().len();
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        !submitted,
        "queue-only work was sent into an autonomous turn that started between ticks"
    );
    assert_eq!(phase, RequestPhase::Queued);
    assert_eq!(
        after_stop, 1,
        "the matching stop must release independent work"
    );
}

#[test]
fn fast_callback_refresh_defers_a_recipient_until_its_native_receipt() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let lead = queue(&mut worker, room, agent, "owned work");
    let native = DeferredNative(Default::default());
    worker.transport = Box::new(native.clone());
    worker.submit_ready().unwrap();
    record(
        &dir,
        Provider::ClaudeCode,
        json!({"hook_event_name":"UserPromptSubmit", "session_id":"session", "prompt_id":"owned-turn", "prompt":"owned work"}),
    );
    queue(&mut worker, room, agent, "queued followup");
    worker.submit_eligible_while(|| true).unwrap();
    let before_receipt = worker.state.request(lead).unwrap().trusted_start_bound;
    native.0.lock().unwrap()[0]
        .1
        .send(Ok(ResponseResult::AgentPrompted {
            agent: native_info("pane", "claude", "session"),
            timestamps: Default::default(),
        }))
        .unwrap();
    worker.finish_native_submissions(false);
    worker.submit_eligible_while(|| true).unwrap();
    let after_receipt = worker.state.request(lead).unwrap().trusted_start_bound;
    let callback_state = format!(
        "{:?}, own turns: {:?}",
        worker.state.request(lead),
        worker.own_turns
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        !before_receipt,
        "callbacks must not overtake an outstanding native write"
    );
    assert!(
        after_receipt,
        "callback reconciliation must resume after the receipt: {callback_state}"
    );
}

#[test]
fn a_fast_callback_checkpoint_failure_prevents_native_input() {
    use crate::messaging::storage::state_store::SaveStage;
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    record(
        &dir,
        Provider::ClaudeCode,
        json!({"hook_event_name":"UserPromptSubmit", "session_id":"session", "prompt_id":"background-turn", "prompt":"autonomous background notification"}),
    );
    let request = queue(
        &mut worker,
        room,
        agent,
        "followup after callback checkpoint",
    );
    let native = DeferredNative(Default::default());
    worker.transport = Box::new(native.clone());
    worker.store.fail_once_at_after(SaveStage::TempWrite, 0);
    let failed = worker.submit_eligible_while(|| true).is_err();
    let sent = !native.0.lock().unwrap().is_empty();
    let durable = JsonStore::new(dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap()
        .request(request)
        .unwrap()
        .phase;
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        failed,
        "callback persistence failure must reach the coordinator"
    );
    assert!(
        !sent,
        "input must not cross an uncommitted callback checkpoint"
    );
    assert_eq!(durable, RequestPhase::Queued);
}
