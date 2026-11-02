use super::*;
use crate::messaging::control::Request as ControlRequest;
use serde_json::json;

#[test]
fn error_pipe_setting_defaults_off_is_dev_only_and_persists_only_current_orchestrators() {
    let root = crate::utils::test_temp::unique_temp_path("bus-error-pipe-settings");
    let mut worker = Worker::open(root.clone(), Box::new(NoTerminals)).unwrap();
    worker.settings_path = Some(root.join("settings.json"));
    let mut state = worker.state.clone();
    let work = state.create_room("work").unwrap();
    let master = state.master_room().unwrap().id;
    let orch = state
        .create_agent(master, "lead", Provider::ClaudeCode, root.clone(), None)
        .unwrap();
    state.bind_orchestrator(orch, work).unwrap();
    let peer = state
        .create_agent(work, "peer", Provider::Codex, root.clone(), None)
        .unwrap();
    worker.save(state).unwrap();
    let (tx, _rx) = mpsc::channel();
    let request = |id: &str, name: &str| ControlRequest {
        id: id.into(),
        method: "settings.error_orchestrator".into(),
        params: json!({"name":name}),
    };
    assert!(
        !worker
            .control_response_with_events(&request("no-dev", "lead"), Some(&tx))
            .ok
    );
    assert!(worker.settings_json().get("error_orchestrator").is_none());
    worker.dev_enabled = true;
    assert_eq!(
        worker.settings_json()["error_orchestrator"],
        serde_json::Value::Null
    );
    assert_eq!(
        worker.settings_json()["error_orchestrators"],
        json!(["lead"])
    );
    let response = worker.control_response_with_events(&request("set", "lead"), Some(&tx));
    assert!(response.ok, "{response:?}");
    assert_eq!(
        serde_json::to_value(settings::load(worker.settings_path.as_ref().unwrap()).unwrap())
            .unwrap()["error_orchestrator"],
        "lead"
    );
    assert!(
        !worker
            .control_response_with_events(
                &request("worker", &worker.state.agent(peer).unwrap().name),
                Some(&tx)
            )
            .ok
    );
    assert_eq!(worker.settings_json()["error_orchestrator"], "lead");
    assert!(
        worker
            .control_response_with_events(&request("off", "off"), Some(&tx))
            .ok
    );
    assert_eq!(
        worker.settings_json()["error_orchestrator"],
        serde_json::Value::Null
    );
    drop(worker);
    std::fs::remove_dir_all(root).unwrap();
}

struct NoTerminals;
impl Transport for NoTerminals {
    fn request(
        &mut self,
        _: Method,
    ) -> Result<ResponseResult, crate::messaging::native::TransportError> {
        panic!("settings and error queueing never need a terminal");
    }
}

fn reporting_worker() -> (Worker, RoomId, AgentId, PathBuf) {
    let root = crate::utils::test_temp::unique_temp_path("bus-error-pipe-delivery");
    let mut worker = Worker::open(root.clone(), Box::new(NoTerminals)).unwrap();
    worker.dev_enabled = true;
    worker.settings_path = Some(root.join("settings.json"));
    let mut state = worker.state.clone();
    let room = state.create_room("work").unwrap();
    let master = state.master_room().unwrap().id;
    let agent = state
        .create_agent(master, "lead", Provider::ClaudeCode, root.clone(), None)
        .unwrap();
    state.bind_orchestrator(agent, room).unwrap();
    state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                ..Default::default()
            },
        )
        .unwrap();
    worker.save(state).unwrap();
    worker
        .set_error_orchestrator(Some("lead".into()), &mpsc::channel().0)
        .unwrap();
    (worker, room, agent, root)
}

fn emit_logged_failure(log: PathBuf, times: usize) {
    use tracing_subscriber::prelude::*;
    let _interest = tracing::Dispatch::new(tracing_subscriber::registry());
    let subscriber =
        tracing_subscriber::registry().with(crate::utils::error_pipe::CaptureLayer::new(log));
    tracing::subscriber::with_default(subscriber, || {
        for _ in 0..times {
            tracing::error!(
                event = "bus.real.failure",
                secret = "full details stay in log",
                "failure details"
            );
        }
    });
}

#[test]
fn error_pipe_queues_one_counted_request_preserves_the_draft_and_off_discards_repeats() {
    let (mut worker, room, agent, root) = reporting_worker();
    let mut state = worker.state.clone();
    state.set_draft_text(room, "KEEP DRAFT").unwrap();
    worker.save(state).unwrap();
    let log = worker.error_log_dir().join("server.log");
    emit_logged_failure(log.clone(), 2);
    worker.refresh_error_pipe_at(100);
    assert_eq!(worker.state.requests().count(), 0);
    worker.refresh_error_pipe_at(1100);
    let requests: Vec<_> = worker.state.requests().collect();
    assert_eq!(requests.len(), 1);
    let request = requests[0];
    assert_eq!(request.agent_id, agent);
    assert_eq!(request.room_id, room);
    assert_eq!(request.prompt.author, Author::Bus);
    assert!(
        request.queue_only,
        "reports never coalesce into an ordinary message"
    );
    assert_eq!(
        request.prompt.text,
        format!(
            "Bus error: bus.real.failure (x2 since last notice). Logs: {}",
            log.display()
        )
    );
    assert_eq!(worker.state.room(room).unwrap().draft.text, "KEEP DRAFT");
    assert!(!worker.state.room(room).unwrap().unseen_to_developer);
    emit_logged_failure(log.clone(), 1);
    worker.refresh_error_pipe_at(2000);
    assert_eq!(worker.state.requests().count(), 1);
    worker
        .set_error_orchestrator(None, &mpsc::channel().0)
        .unwrap();
    emit_logged_failure(log, 1);
    worker.refresh_error_pipe_at(70000);
    assert_eq!(worker.state.requests().count(), 1);
    drop(worker);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn error_pipe_rejected_or_uncertain_reports_close_without_retry_or_later_agent_error() {
    for outcome in [
        SubmissionOutcome::DefinitelyRejected {
            message: "broken pipe".into(),
        },
        SubmissionOutcome::Uncertain {
            message: "lost response".into(),
        },
    ] {
        let (mut worker, room, agent, root) = reporting_worker();
        let mut state = worker.state.clone();
        let report = state
            .submit_message_with(
                room,
                Draft {
                    text: "Bus error: failure (x1 since last notice). Logs: /logs/client.log"
                        .into(),
                    files: vec![],
                    recipient_ids: [agent].into(),
                },
                Author::Bus,
                1,
                true,
            )
            .unwrap()[0];
        state.begin_submission(report, "launch", 0).unwrap();
        state.record_submission(report, outcome).unwrap();
        assert!(
            state.request(report).unwrap().settled(),
            "a failed report is consumed"
        );
        assert!(state.queued_requests(agent).is_empty(), "no report retry");
        assert_eq!(
            state.agent(agent).unwrap().actionable_error,
            None,
            "no deferred client warning can feed back"
        );
        worker.save(state).unwrap();
        drop(worker);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn error_pipe_persistence_failure_never_enters_the_storage_pause_feedback_path() {
    let (mut worker, _, _, root) = reporting_worker();
    let mut state = worker.state.clone();
    state.create_room("cannot-save").unwrap();
    std::fs::remove_file(root.join("state.json")).unwrap();
    std::fs::create_dir(root.join("state.json")).unwrap();
    let _guard = crate::utils::error_pipe::suppress(true);
    assert!(worker.save(state).is_err());
    assert!(
        worker.storage_pause.is_none(),
        "report-save failure must not create a later retry/projection warning"
    );
    assert!(worker.error.is_none());
    drop(worker);
    std::fs::remove_dir_all(root).unwrap();
}
