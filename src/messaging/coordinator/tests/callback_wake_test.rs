use super::*;
use std::time::Instant;

struct CallbackNative {
    info: schema::AgentInfo,
}
impl Transport for CallbackNative {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::AgentListWithDetectionContext(_) => Ok(ResponseResult::AgentList {
                agents: vec![self.info.clone()],
            }),
            Method::AgentPromptIfIdleAtGeneration(_) => Ok(ResponseResult::AgentPrompted {
                agent: self.info.clone(),
                timestamps: Default::default(),
            }),
            _ => Ok(ResponseResult::Ok {}),
        }
    }
}

struct TickWitness {
    native: CallbackNative,
    path: PathBuf,
    request: RequestId,
    polls: usize,
    seen: mpsc::Sender<bool>,
}
impl Transport for TickWitness {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        if matches!(method, Method::AgentListWithDetectionContext(_)) {
            self.polls += 1;
            if self.polls == 2 {
                let saved = JsonStore::new(self.path.clone()).load().unwrap().unwrap();
                self.seen
                    .send(saved.request(self.request).unwrap().trusted_start_bound)
                    .unwrap();
            }
        }
        self.native.request(method)
    }
}

#[test]
fn durable_started_callback_wakes_the_coordinator_before_the_next_tick() {
    let _providers = provider_binaries();
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let mut info = native_info("pane", "claude", "session");
    info.name = Some(format!("bus-r{}-a{}", room.0, agent.0));
    info.interactive_ready = true;
    worker.transport = Box::new(CallbackNative { info });
    let request = queue(&mut worker, room, agent, "wake this callback");
    worker.submit_ready().unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Active
    );
    let (seen, witness) = mpsc::channel();
    worker.transport = Box::new(TickWitness {
        native: CallbackNative {
            info: native_info("pane", "claude", "session"),
        },
        path: dir.join("state.json"),
        request,
        polls: 0,
        seen,
    });
    let (saved, observed) = mpsc::channel();
    worker.test_save_observations = Some(saved);
    let (commands, receiver) = mpsc::sync_channel(256);
    let control = crate::messaging::control::server::start(&dir, commands.clone()).unwrap();
    let snapshots = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
    let shared = snapshots.clone();
    let (events, _) = mpsc::channel();
    let thread = std::thread::spawn(move || worker.run(receiver, events, shared));
    loop {
        if observed
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .operation
            == "tick"
        {
            break;
        }
    }
    record(
        &dir,
        Provider::ClaudeCode,
        json!({"hook_event_name":"UserPromptSubmit", "session_id":"session", "prompt_id":"turn", "prompt":"wake this callback"}),
    );
    let before_tick = witness.recv_timeout(Duration::from_secs(3)).unwrap();
    commands.send((1, BusCommand::Shutdown)).unwrap();
    thread.join().unwrap();
    drop(control);
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        before_tick,
        "durably published Started was still unbound when the next full native poll began"
    );
}

fn prepared(provider: Provider) -> (Worker, AgentId, RequestId, PathBuf) {
    let (mut worker, agent, room, dir, _) = fixture(provider, vec![]);
    let mut info = native_info("pane", launch::provider_kind(provider), "session");
    info.name = Some(format!("bus-r{}-a{}", room.0, agent.0));
    info.interactive_ready = true;
    worker.transport = Box::new(CallbackNative { info });
    let request = queue(&mut worker, room, agent, "callback neighbours");
    worker.submit_ready().unwrap();
    (worker, agent, request, dir)
}

fn started(provider: Provider) -> serde_json::Value {
    match provider {
        Provider::Cursor => {
            json!({"hook_event_name":"beforeSubmitPrompt", "conversation_id":"session", "generation_id":"turn", "prompt":"callback neighbours"})
        }
        Provider::Kimi => {
            json!({"hook_event_name":"TurnStarted", "session_id":"session", "turn_id":1, "origin_kind":"user", "prompt":"callback neighbours"})
        }
        Provider::Grok => {
            json!({"hookEventName":"user_prompt_submit", "sessionId":"session", "promptId":"turn", "prompt":"callback neighbours"})
        }
        _ => {
            json!({"hook_event_name":"UserPromptSubmit", "session_id":"session", "turn_id":"turn", "prompt_id":"turn", "prompt":"callback neighbours"})
        }
    }
}

#[test]
fn callback_wake_preserves_each_provider_parser_and_durable_acknowledgement() {
    let _providers = provider_binaries();
    for provider in [
        Provider::ClaudeCode,
        Provider::ClaudeDs,
        Provider::Codex,
        Provider::Cursor,
        Provider::Grok,
        Provider::Kimi,
    ] {
        let (mut worker, _, request, dir) = prepared(provider);
        record(&dir, provider, started(provider));
        worker.queue_callback_wake("launch");
        worker.consume_woken_callbacks().unwrap();
        assert!(
            worker.state.request(request).unwrap().trusted_start_bound,
            "{provider:?}"
        );
        assert!(callbacks::records(&dir.join("callbacks/launch"))
            .unwrap()
            .is_empty());
        let durable = JsonStore::new(dir.join("state.json"))
            .load()
            .unwrap()
            .unwrap();
        assert!(durable.request(request).unwrap().trusted_start_bound);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn callback_wake_ignores_stale_launch_hints_after_replacement_and_generation_change() {
    let (mut worker, agent, request, dir) = prepared(Provider::ClaudeCode);
    record(&dir, Provider::ClaudeCode, started(Provider::ClaudeCode));
    let mut state = worker.state.clone();
    let mut identity = state.agent(agent).unwrap().runtime_identity.clone();
    identity.launch_id = Some("replacement".into());
    identity.terminal_id = Some("replacement-terminal".into());
    state.set_agent_runtime_identity(agent, identity).unwrap();
    worker.save(state).unwrap();
    worker.serving_generation += 1;
    for hint in ["launch", "../launch", "unrelated"] {
        worker.queue_callback_wake(hint);
    }
    worker.consume_woken_callbacks().unwrap();
    assert!(worker.callback_wakes.is_empty());
    assert!(!worker.state.request(request).unwrap().trusted_start_bound);
    assert_eq!(
        callbacks::records(&dir.join("callbacks/launch"))
            .unwrap()
            .len(),
        1
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn callback_wake_duplicate_burst_has_one_final_and_survives_reopen() {
    let (mut worker, _, request, dir) = prepared(Provider::ClaudeCode);
    let start = started(Provider::ClaudeCode);
    std::thread::scope(|scope| {
        for _ in 0..16 {
            let dir = &dir;
            let start = start.clone();
            scope.spawn(move || record(dir, Provider::ClaudeCode, start));
        }
    });
    worker.queue_callback_wake("launch");
    worker.consume_woken_callbacks().unwrap();
    assert!(worker.state.request(request).unwrap().trusted_start_bound);
    let mut state = worker.state.clone();
    let message = state.request(request).unwrap().prompt.id;
    state
        .answer_message(
            message,
            state.request(request).unwrap().agent_id,
            "explicit answer".into(),
            io::now_ms(),
        )
        .unwrap();
    worker.save(state).unwrap();
    let stop = json!({"hook_event_name":"Stop", "session_id":"session", "prompt_id":"turn", "last_assistant_message":"one final"});
    std::thread::scope(|scope| {
        for _ in 0..16 {
            let dir = &dir;
            let stop = stop.clone();
            scope.spawn(move || record(dir, Provider::ClaudeCode, stop));
        }
    });
    worker.queue_callback_wake("launch");
    worker.consume_woken_callbacks().unwrap();
    let reply = worker
        .state
        .request(request)
        .unwrap()
        .completed_reply
        .clone();
    assert!(reply.is_some());
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Completed
    );
    drop(worker);
    let mut worker = Worker::open(
        dir.clone(),
        Box::new(CallbackNative {
            info: native_info("pane", "claude", "session"),
        }),
    )
    .unwrap();
    // A duplicate published after the previous envelope was acknowledged still
    // cannot create a second reply, including after the coordinator restarts.
    record(&dir, Provider::ClaudeCode, stop);
    worker.queue_callback_wake("launch");
    worker.consume_woken_callbacks().unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().completed_reply,
        reply
    );
    assert!(callbacks::records(&dir.join("callbacks/launch"))
        .unwrap()
        .is_empty());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn callback_wake_retries_durable_stages_after_append_lock_contention() {
    let (mut worker, _, request, dir) = prepared(Provider::ClaudeCode);
    let lock = callbacks::try_append_lock(&dir.join("callbacks/launch/append.lock")).unwrap();
    record(&dir, Provider::ClaudeCode, started(Provider::ClaudeCode));
    worker.queue_callback_wake("launch");
    worker.consume_woken_callbacks().unwrap();
    assert!(!worker.state.request(request).unwrap().trusted_start_bound);
    assert!(!worker.callback_wakes.is_empty());
    drop(lock);
    worker.consume_woken_callbacks().unwrap();
    assert!(worker.state.request(request).unwrap().trusted_start_bound);
    assert!(worker.callback_wakes.is_empty());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn callback_wake_save_failure_retains_spool_and_recovers_without_double_application() {
    let (mut worker, _, request, dir) = prepared(Provider::ClaudeCode);
    record(&dir, Provider::ClaudeCode, started(Provider::ClaudeCode));
    worker
        .store
        .fail_once_at(crate::messaging::storage::state_store::SaveStage::TempWrite);
    worker.queue_callback_wake("launch");
    assert!(worker.consume_woken_callbacks().is_err());
    assert_eq!(
        callbacks::records(&dir.join("callbacks/launch"))
            .unwrap()
            .len(),
        1
    );
    assert!(!worker.state.request(request).unwrap().trusted_start_bound);
    let (events, _) = mpsc::channel();
    worker.retry_storage(Instant::now() + Duration::from_secs(2), &events);
    worker.consume_woken_callbacks().unwrap();
    assert!(worker.state.request(request).unwrap().trusted_start_bound);
    assert!(callbacks::records(&dir.join("callbacks/launch"))
        .unwrap()
        .is_empty());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

struct PendingCallbackNative {
    receipt: Option<mpsc::Sender<crate::messaging::native::NativeResult>>,
}
impl Transport for PendingCallbackNative {
    fn request(&mut self, _: Method) -> Result<ResponseResult, TransportError> {
        panic!("unexpected synchronous IO");
    }
    fn submit(&mut self, _: Method) -> crate::messaging::native::NativeCall {
        let (tx, rx) = mpsc::channel();
        self.receipt = Some(tx);
        crate::messaging::native::NativeCall::Pending(rx)
    }
}

#[test]
fn callback_wake_waits_for_in_flight_native_receipt() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let native = PendingCallbackNative { receipt: None };
    // Keep the receipt outside the boxed transport through a separate channel.
    let (tx, rx) = mpsc::channel();
    worker.transport = Box::new(ReceiptBridge { tx, native });
    let request = queue(&mut worker, room, agent, "callback neighbours");
    worker.submit_ready().unwrap();
    let receipt = rx.recv().unwrap();
    record(&dir, Provider::ClaudeCode, started(Provider::ClaudeCode));
    worker.queue_callback_wake("launch");
    worker.consume_woken_callbacks().unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Submitting
    );
    assert_eq!(
        callbacks::records(&dir.join("callbacks/launch"))
            .unwrap()
            .len(),
        1
    );
    receipt
        .send(Ok(ResponseResult::AgentPrompted {
            agent: native_info("pane", "claude", "session"),
            timestamps: Default::default(),
        }))
        .unwrap();
    worker.finish_native_submissions(false);
    worker.consume_woken_callbacks().unwrap();
    assert!(worker.state.request(request).unwrap().trusted_start_bound);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

struct ReceiptBridge {
    tx: mpsc::Sender<mpsc::Sender<crate::messaging::native::NativeResult>>,
    native: PendingCallbackNative,
}
impl Transport for ReceiptBridge {
    fn request(&mut self, _: Method) -> Result<ResponseResult, TransportError> {
        panic!("unexpected synchronous IO");
    }
    fn submit(&mut self, m: Method) -> crate::messaging::native::NativeCall {
        let call = self.native.submit(m);
        self.tx.send(self.native.receipt.take().unwrap()).unwrap();
        call
    }
}

#[test]
fn callback_wake_coalesces_bursts_without_crossing_mutation_barriers() {
    let (mut worker, agent, _, dir) = prepared(Provider::ClaudeCode);
    let mut pending = std::collections::VecDeque::from([
        (0, BusCommand::CallbackReady("launch".into())),
        (0, BusCommand::CallbackReady("launch".into())),
        (3, BusCommand::DeleteAgent(agent)),
        (0, BusCommand::CallbackReady("launch".into())),
    ]);
    worker.coalesce_callback_wakes(&mut pending);
    assert_eq!(worker.callback_wakes.len(), 1);
    assert!(matches!(
        pending.front(),
        Some((3, BusCommand::DeleteAgent(_)))
    ));
    assert_eq!(pending.len(), 2);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn callback_wake_full_hint_queue_never_loses_the_durable_event() {
    let (mut worker, _, request, dir) = prepared(Provider::ClaudeCode);
    let (tx, _rx) = mpsc::sync_channel(0);
    let server = crate::messaging::control::server::start(&dir, tx).unwrap();
    let at = Instant::now();
    record(&dir, Provider::ClaudeCode, started(Provider::ClaudeCode));
    assert!(at.elapsed() < Duration::from_millis(250));
    assert_eq!(
        callbacks::records(&dir.join("callbacks/launch"))
            .unwrap()
            .len(),
        1
    );
    drop(server);
    // The normal periodic consumer remains a recovery path for an undelivered hint.
    worker
        .consume_callbacks(
            worker.state.request(request).unwrap().agent_id,
            &dir.join("callbacks/launch"),
        )
        .unwrap();
    assert!(worker.state.request(request).unwrap().trusted_start_bound);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn callback_wake_closed_publisher_hint_is_safe_to_admit_without_a_mutation() {
    use std::io::Write;
    let (worker, _, _, dir) = prepared(Provider::ClaudeCode);
    let (tx, rx) = mpsc::sync_channel(4);
    let control = crate::messaging::control::server::start(&dir, tx).unwrap();
    let mut stream = crate::platform::ipc::connect_local_stream_timeout(
        &dir.join("control.sock"),
        Duration::from_millis(100),
    )
    .unwrap();
    stream
        .write_all(b"{\"callback_wake\":\"launch\"}\n")
        .unwrap();
    drop(stream);
    assert!(
        matches!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), (0, BusCommand::CallbackReady(launch)) if launch == "launch")
    );
    drop(control);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn callback_wake_does_not_spend_grok_reconciliation_budget_on_stale_idle() {
    let (mut worker, agent, _, dir) = prepared(Provider::Grok);
    record(&dir, Provider::Grok, started(Provider::Grok));
    worker.queue_callback_wake("launch");
    worker.consume_woken_callbacks().unwrap();
    // The cached native observation is still Idle while the provider runs its
    // synchronous Stop gate. A tick would refresh it before reconciling.
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, io::now_ms())
        .unwrap();
    record(
        &dir,
        Provider::Grok,
        json!({"hookEventName":"stop", "sessionId":"session", "promptId":"turn", "reason":"end_turn"}),
    );
    worker.queue_callback_wake("launch");
    worker.consume_woken_callbacks().unwrap();
    let retries = std::fs::read_dir(dir.join("callbacks/launch"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "grok-retry"))
        .count();
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(retries, 0, "a wake used stale Idle and started the two-second Grok retry budget before fresh native proof");
}

#[test]
fn callback_wake_keeps_kimi_wire_reconciliation_for_the_fresh_poll() {
    let (mut worker, _, request, dir) = prepared(Provider::Kimi);
    record(&dir, Provider::Kimi, started(Provider::Kimi));
    worker.queue_callback_wake("launch");
    worker.consume_woken_callbacks().unwrap();
    record(
        &dir,
        Provider::Kimi,
        json!({"hook_event_name":"Stop", "session_id":"session"}),
    );
    worker.queue_callback_wake("launch");
    worker.consume_woken_callbacks().unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Active
    );
    assert_eq!(
        callbacks::records(&dir.join("callbacks/launch"))
            .unwrap()
            .len(),
        1
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn callback_wake_cursor_response_waits_for_its_matching_stop() {
    let (mut worker, _, request, dir) = prepared(Provider::Cursor);
    record(&dir, Provider::Cursor, started(Provider::Cursor));
    worker.queue_callback_wake("launch");
    worker.consume_woken_callbacks().unwrap();
    record(
        &dir,
        Provider::Cursor,
        json!({"hook_event_name":"afterAgentResponse", "conversation_id":"session", "generation_id":"turn", "text":"one response"}),
    );
    worker.queue_callback_wake("launch");
    worker.consume_woken_callbacks().unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Active
    );
    record(
        &dir,
        Provider::Cursor,
        json!({"hook_event_name":"stop", "conversation_id":"session", "generation_id":"turn", "status":"completed"}),
    );
    worker.queue_callback_wake("launch");
    worker.consume_woken_callbacks().unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Completed
    );
    assert!(
        worker
            .state
            .request(request)
            .unwrap()
            .completed_reply
            .is_none(),
        "turn finals must not publish replies"
    );
    assert!(callbacks::records(&dir.join("callbacks/launch"))
        .unwrap()
        .is_empty());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
