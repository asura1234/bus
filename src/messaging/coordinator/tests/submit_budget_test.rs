use super::*;
use crate::messaging::native::BusTransport;
use crate::protocol::api::client::{ApiClientError, ConnectionTarget};

/// The native reply and the socket deadline race on a virtual timeline. Only
/// the timeout supplied by the real BusTransport decides which arrives first.
struct DelayedNativeReply {
    client: BusTransport,
    delay: Duration,
    calls: Arc<Mutex<Vec<Duration>>>,
}

impl Transport for DelayedNativeReply {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        let delay = self.delay;
        let calls = self.calls.clone();
        self.client
            .request_using(method, move |_, request, timeout| {
                assert!(matches!(request.method, Method::AgentPromptIfIdle(_)));
                calls.lock().unwrap().push(timeout);
                if delay >= timeout {
                    return Err(ApiClientError::Io(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "native response arrived after the read deadline",
                    )));
                }
                Ok(json!({
                    "id": request.id,
                    "error": {
                        "code": crate::protocol::api::AGENT_PROMPT_NOT_SHOWN,
                        "message": crate::protocol::api::AGENT_PROMPT_NOT_SHOWN_MESSAGE
                    }
                }))
            })
    }
}

fn delayed_worker(
    delay: Duration,
) -> (Worker, AgentId, RoomId, PathBuf, Arc<Mutex<Vec<Duration>>>) {
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let calls = Arc::new(Mutex::new(Vec::new()));
    worker.transport = Box::new(DelayedNativeReply {
        client: BusTransport::new(ConnectionTarget::SocketPath(dir.join("unused.sock"))),
        delay,
        calls: calls.clone(),
    });
    (worker, agent, room, dir, calls)
}

#[test]
fn native_not_shown_after_3100ms_reaches_coordinator_and_is_retried() {
    let (mut worker, agent, room, dir, calls) = delayed_worker(Duration::from_millis(3100));
    let request = queue(&mut worker, room, agent, "resume the interrupted task");
    worker.submit_ready().unwrap();
    assert_eq!(calls.lock().unwrap().len(), 1);
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Queued,
        "the late definite rejection must not strand ownership in Submitting"
    );
    assert!(!worker.state.request(request).unwrap().uncertain_outcome);
    assert_eq!(worker.state.agent(agent).unwrap().current_request, None);
    assert_eq!(
        worker
            .state
            .agent(agent)
            .unwrap()
            .delivery_rejection
            .as_deref(),
        Some("prompt_not_shown")
    );
    worker.submit_ready().unwrap();
    assert_eq!(calls.lock().unwrap().len(), 1, "retain retry backoff");
    for at in worker.withdrawn_at.values_mut() {
        *at -= crate::messaging::coordinator::delivery::PROMPT_NOT_SHOWN_RETRY;
    }
    worker.submit_ready().unwrap();
    assert_eq!(calls.lock().unwrap().len(), 2, "retry the withdrawn paste");
    assert!(!worker.state.request(request).unwrap().uncertain_outcome);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_genuinely_lost_native_reply_remains_uncertain_and_is_never_retried() {
    let (mut worker, agent, room, dir, calls) = delayed_worker(Duration::from_secs(60));
    let request = queue(&mut worker, room, agent, "do not duplicate this prompt");
    worker.submit_ready().unwrap();
    assert!(worker.state.request(request).unwrap().uncertain_outcome);
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Submitting
    );
    assert_eq!(
        worker.state.agent(agent).unwrap().current_request,
        Some(request)
    );
    worker.submit_ready().unwrap();
    assert_eq!(calls.lock().unwrap().len(), 1);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
