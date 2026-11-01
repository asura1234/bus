//! Behavioral coverage for unchanged control reads between native polls.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize};

fn status_request(
    message: PromptId,
) -> (
    BusCommand,
    mpsc::Receiver<crate::messaging::control::Response>,
) {
    let (reply, receiver) = mpsc::sync_channel(1);
    (
        BusCommand::Control(crate::messaging::control::server::ControlCall {
            request: crate::messaging::control::Request {
                id: format!("poll-{}", io::now_ns()),
                method: "message.status".into(),
                params: json!({"message":message.0.to_string()}),
            },
            reply,
            deadline: None,
            _poll_permit: None,
        }),
        receiver,
    )
}

struct FloodNative {
    info: schema::AgentInfo,
    polls: Arc<AtomicUsize>,
    submitted: mpsc::Sender<()>,
}

impl Transport for FloodNative {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::AgentList(_) | Method::AgentListWithDetectionContext(_) => {
                let poll = self.polls.fetch_add(1, Ordering::SeqCst);
                self.info.launch_pending = poll == 0;
                self.info.interactive_ready = poll > 0;
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
            _ => Ok(ResponseResult::Ok {}),
        }
    }
}

#[test]
fn successful_status_flood_keeps_the_delivery_poll_due() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let request = queue(&mut worker, room, agent, "deliver while followers wait");
    let message = worker.state.request(request).unwrap().prompt.id;
    let polls = Arc::new(AtomicUsize::new(0));
    let (submitted, observed) = mpsc::channel();
    worker.transport = Box::new(FloodNative {
        info: native_info("pane", "codex", "session"),
        polls: Arc::clone(&polls),
        submitted,
    });
    let (commands, receiver) = mpsc::sync_channel(256);
    let (events, _) = mpsc::channel();
    let snapshots = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
    let running = Arc::new(AtomicBool::new(true));
    let answered = Arc::new(AtomicUsize::new(0));
    let thread = std::thread::spawn(move || worker.run(receiver, events, snapshots));
    let followers: Vec<_> = (0..20)
        .map(|_| {
            let commands = commands.clone();
            let running = Arc::clone(&running);
            let answered = Arc::clone(&answered);
            std::thread::spawn(move || {
                while running.load(Ordering::Acquire) {
                    let (call, reply) = status_request(message);
                    if commands.send((1, call)).is_err() {
                        break;
                    }
                    if reply
                        .recv_timeout(Duration::from_secs(5))
                        .is_ok_and(|r| r.ok)
                    {
                        answered.fetch_add(1, Ordering::SeqCst);
                    }
                }
            })
        })
        .collect();
    let result = observed.recv_timeout(Duration::from_secs(5));
    let count = answered.load(Ordering::SeqCst);
    running.store(false, Ordering::Release);
    for follower in followers {
        follower.join().unwrap();
    }
    commands.send((2, BusCommand::Shutdown)).unwrap();
    thread.join().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        result.is_ok(),
        "continuous successful reads must not starve a queued delivery"
    );
    assert!(
        polls.load(Ordering::SeqCst) >= 2,
        "the first poll was not ready"
    );
    assert!(
        count >= 20,
        "exercise unchanged reads between two polls: {count}"
    );
}
