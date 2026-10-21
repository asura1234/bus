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
