//! A read can fail a required durable transition without advancing revision.
use super::*;
use crate::messaging::coordinator::{Draft, PromptId, Provider};
use crate::messaging::native::{Transport, TransportError};
use crate::protocol::api::schema::{Method, ResponseResult};
use serde_json::json;

struct NoNative;
impl Transport for NoNative {
    fn request(&mut self, _method: Method) -> Result<ResponseResult, TransportError> {
        panic!("expiry must not access the native server");
    }
}

#[test]
fn an_error_setting_status_read_requires_a_snapshot_refresh() {
    let dir = crate::utils::test_temp::unique_temp_path("status-read-error");
    let mut worker = Worker::open(dir.clone(), Box::new(NoNative)).unwrap();
    let mut state = worker.state.clone();
    let room = state.create_room("status-read-error").unwrap();
    let agent = state
        .create_agent(room, "worker", Provider::Codex, dir.clone(), None)
        .unwrap();
    state.confirm_hook_setup(agent).unwrap();
    state
        .observe_status(agent, crate::messaging::model::RuntimeStatus::Idle, 1)
        .unwrap();
    let requests = state
        .submit_message_from(
            room,
            Draft {
                text: "expired queued work".into(),
                files: vec![],
                recipient_ids: [agent].into(),
            },
            crate::messaging::model::Author::Developer,
            crate::messaging::storage::io::now_ms() - crate::messaging::model::QUEUED_STALL_MS - 1,
        )
        .unwrap();
    let message: PromptId = state.request(requests[0]).unwrap().prompt.id;
    worker.save(state).unwrap();
    worker
        .store
        .fail_once_at(crate::messaging::storage::state_store::SaveStage::TempWrite);
    let revision = worker.revision;
    let snapshots = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
    let (events, _) = mpsc::channel();
    let (reply, response) = mpsc::sync_channel(1);
    let call = ControlCall {
        request: crate::messaging::control::Request {
            id: "expiry-read".into(),
            method: "message.status".into(),
            params: json!({"message":message.0.to_string()}),
        },
        reply,
        deadline: None,
        _poll_permit: None,
    };
    let skipped = worker.serve_control_without_refresh(
        call,
        &events,
        &snapshots,
        Instant::now() + std::time::Duration::from_secs(30),
    );
    assert!(!response.recv().unwrap().ok);
    assert_eq!(
        worker.revision, revision,
        "the failed save changes no model revision"
    );
    assert!(worker.error.is_some());
    assert!(
        !skipped,
        "the read's new error must reach the UI refresh path"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unchanged_reads_only_defer_refresh_until_the_delivery_poll_is_due() {
    let dir = crate::utils::test_temp::unique_temp_path("status-read-due");
    let mut worker = Worker::open(dir.clone(), Box::new(NoNative)).unwrap();
    let snapshots = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
    let (events, _) = mpsc::channel();
    for due in [false, true] {
        let (reply, response) = mpsc::sync_channel(1);
        let call = ControlCall {
            request: crate::messaging::control::Request {
                id: format!("read-due-{due}"),
                method: "state".into(),
                params: json!({}),
            },
            reply,
            deadline: None,
            _poll_permit: None,
        };
        let now = Instant::now();
        let next_poll = if due {
            now
        } else {
            now + std::time::Duration::from_secs(30)
        };
        let skipped = worker.serve_control_without_refresh(call, &events, &snapshots, next_poll);
        assert!(response.recv().unwrap().ok);
        assert_eq!(
            skipped, !due,
            "an unchanged read cannot renew a due delivery deadline"
        );
    }
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
