//! Control calls under a saturated coordinator: who runs first and what is skipped.
use super::*;
use std::time::Instant;

struct NoAgents;

impl Transport for NoAgents {
    fn request(&mut self, _: Method) -> Result<ResponseResult, TransportError> {
        Ok(ResponseResult::AgentList { agents: vec![] })
    }
}

fn data_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "bus-{name}-{}-{}",
        std::process::id(),
        io::now_ns()
    ))
}

fn call(
    id: &str,
    method: &str,
    params: serde_json::Value,
    deadline: Option<Instant>,
    reply: &mpsc::SyncSender<control::Response>,
) -> (u64, BusCommand) {
    (
        1,
        BusCommand::Control(control::ControlCall {
            request: crate::messaging::control::Request {
                id: id.into(),
                method: method.into(),
                params,
            },
            reply: reply.clone(),
            deadline,
            _poll_permit: None,
        }),
    )
}

/// Runs a real coordinator over `queued`, all enqueued before it starts, and
/// returns the ids of the responses in the order it sent them.
fn run_queued(
    name: &str,
    queued: impl FnOnce(&mpsc::SyncSender<control::Response>) -> Vec<(u64, BusCommand)>,
) -> Vec<String> {
    let dir = data_dir(name);
    let worker = Worker::open(dir.clone(), Box::new(NoAgents)).unwrap();
    let (replies, answered) = mpsc::sync_channel(64);
    let (commands, receiver) = mpsc::sync_channel(64);
    for command in queued(&replies) {
        commands.send(command).unwrap();
    }
    commands.send((2, BusCommand::Shutdown)).unwrap();
    drop(replies);
    let snapshot = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
    let (events, _) = mpsc::channel();
    std::thread::spawn(move || worker.run(receiver, events, snapshot))
        .join()
        .unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    answered.try_iter().map(|response| response.id).collect()
}

#[test]
fn an_interactive_read_runs_before_queued_status_polls() {
    let order = run_queued("interactive-first", |reply| {
        let mut queued: Vec<_> = (0..4)
            .map(|n| {
                call(
                    &format!("poll-{n}"),
                    "message.status",
                    json!({"message": "1"}),
                    None,
                    reply,
                )
            })
            .collect();
        queued.push(call("state", "state", json!({}), None, reply));
        queued
    });
    assert_eq!(
        order.first().map(String::as_str),
        Some("state"),
        "{order:?}"
    );
    assert_eq!(order.len(), 5, "{order:?}");
}

#[test]
fn a_read_whose_caller_already_timed_out_is_skipped_but_a_mutation_still_runs() {
    let past = Instant::now() - Duration::from_secs(1);
    let order = run_queued("expired-reads", |reply| {
        vec![
            call(
                "stale-poll",
                "message.status",
                json!({"message": "1"}),
                Some(past),
                reply,
            ),
            call("stale-state", "state", json!({}), Some(past), reply),
            call(
                "stale-create",
                "room.create",
                json!({"name": "late"}),
                Some(past),
                reply,
            ),
            call(
                "fresh-state",
                "state",
                json!({}),
                Some(Instant::now() + Duration::from_secs(60)),
                reply,
            ),
        ]
    });
    assert_eq!(order, ["stale-create", "fresh-state"]);
}

#[cfg(target_os = "macos")]
#[test]
fn the_coordinator_thread_runs_at_user_interactive_qos() {
    struct Qos(mpsc::Sender<libc::qos_class_t>);
    impl Transport for Qos {
        fn request(&mut self, _: Method) -> Result<ResponseResult, TransportError> {
            let mut class = libc::qos_class_t::QOS_CLASS_UNSPECIFIED;
            let mut priority = 0;
            // SAFETY: reads this thread's own QoS into valid locals.
            unsafe {
                libc::pthread_get_qos_class_np(libc::pthread_self(), &mut class, &mut priority)
            };
            let _ = self.0.send(class);
            Ok(ResponseResult::AgentList { agents: vec![] })
        }
    }
    let dir = data_dir("coordinator-qos");
    let (seen, observed) = mpsc::channel();
    let worker = Worker::open(dir.clone(), Box::new(Qos(seen))).unwrap();
    let (commands, receiver) = mpsc::sync_channel(4);
    let snapshot = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
    let (events, _) = mpsc::channel();
    let thread = std::thread::spawn(move || worker.run(receiver, events, snapshot));
    let class = observed.recv_timeout(Duration::from_secs(5));
    commands.send((2, BusCommand::Shutdown)).unwrap();
    thread.join().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        matches!(class, Ok(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE)),
        "{class:?}"
    );
}

#[path = "control_fairness_test.rs"]
mod control_fairness_tests;

#[test]
fn a_dequeued_control_log_caps_the_id_without_changing_the_response_id() {
    let dir = data_dir("dequeued-log-id");
    let worker = Worker::open(dir.clone(), Box::new(NoAgents)).unwrap();
    let id = "r".repeat(200 * 1024);
    let (replies, answered) = mpsc::sync_channel(2);
    let (commands, receiver) = mpsc::sync_channel(2);
    commands
        .send(call(
            &id,
            "state",
            json!({}),
            Some(Instant::now() + Duration::from_secs(15)),
            &replies,
        ))
        .unwrap();
    commands.send((2, BusCommand::Shutdown)).unwrap();
    let snapshot = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
    let (events, _) = mpsc::channel();
    // Keep callsite interest global while other libtest threads exercise the
    // same log site without a thread-local subscriber.
    let _callsite_registration = tracing::Dispatch::new(tracing_subscriber::registry());
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run_filtered("bus=debug", || worker.run(receiver, events, snapshot));
    let response = answered.recv_timeout(Duration::from_secs(5)).unwrap();
    let text = capture.text();
    let record = text
        .lines()
        .find(|line| line.contains("bus.control.command.dequeued"))
        .expect("dequeue diagnostic");
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(
        response.id, id,
        "log truncation must not change response correlation"
    );
    assert!(
        record.contains(&"r".repeat(128)),
        "the useful ID prefix must remain"
    );
    assert!(
        !record.contains(&"r".repeat(129)),
        "dequeue record contains an uncapped {} byte ID",
        record.len()
    );
}

#[test]
fn a_delete_reply_publishes_removal_before_the_following_tick() {
    struct BlockPoll {
        entered: mpsc::Sender<()>,
        resume: mpsc::Receiver<()>,
    }
    impl Transport for BlockPoll {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            assert!(matches!(method, Method::AgentList(_)));
            self.entered.send(()).unwrap();
            self.resume.recv_timeout(Duration::from_secs(10)).unwrap();
            Ok(ResponseResult::AgentList { agents: vec![] })
        }
    }
    for fail_close in [false, true] {
        let dir = data_dir("delete-publish");
        let (entered, polling) = mpsc::channel();
        let (resume, resumed) = mpsc::channel();
        let mut worker = Worker::open(
            dir.clone(),
            Box::new(BlockPoll {
                entered,
                resume: resumed,
            }),
        )
        .unwrap();
        let mut state = worker.state.clone();
        let room = state.create_room("deleting").unwrap();
        let agent = state
            .create_agent(room, "worker", Provider::Codex, dir.clone(), None)
            .unwrap();
        if fail_close {
            state
                .set_agent_runtime_identity(
                    agent,
                    AgentRuntimeIdentity {
                        launch_id: Some("unconfirmed-launch".into()),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        worker.save(state).unwrap();
        let snapshots = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
        let shared = Arc::clone(&snapshots);
        let (replies, answered) = mpsc::sync_channel(1);
        let (commands, receiver) = mpsc::sync_channel(4);
        commands
            .send(call(
                "delete",
                "agent.delete",
                json!({"agent":agent.0.to_string(),"confirm":true}),
                None,
                &replies,
            ))
            .unwrap();
        let (events, _) = mpsc::channel();
        let thread = std::thread::spawn(move || worker.run(receiver, events, snapshots));
        let response = answered.recv_timeout(Duration::from_secs(10)).unwrap();
        polling.recv_timeout(Duration::from_secs(10)).unwrap();
        // Keep the next tick blocked, as a slow native API or callback pass would.
        let shown = shared
            .lock()
            .unwrap()
            .state
            .agent(agent)
            .map(|a| a.deletion_pending);
        let saved = JsonStore::new(dir.join("state.json"))
            .load()
            .unwrap()
            .unwrap();
        commands.send((2, BusCommand::Shutdown)).unwrap();
        resume.send(()).unwrap();
        thread.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        assert_eq!(response.ok, !fail_close, "{response:?}");
        assert_eq!(
            saved.agent(agent).map(|a| a.deletion_pending),
            fail_close.then_some(true),
            "ok must mean durable removal; an unconfirmed close must retain pending intent"
        );
        assert_eq!(shown, fail_close.then_some(true), "delete outcome was returned before its removal or pending intent reached the UI snapshot");
    }
}

#[test]
fn a_read_queued_behind_polls_never_overtakes_an_earlier_mutation() {
    let order = run_queued("read-after-mutation", |reply| {
        vec![
            call(
                "poll",
                "message.status",
                json!({"message": "1"}),
                None,
                reply,
            ),
            call(
                "create",
                "room.create",
                json!({"name": "first"}),
                None,
                reply,
            ),
            call("state", "state", json!({}), None, reply),
        ]
    });
    assert_eq!(order, ["create", "state", "poll"]);
}
