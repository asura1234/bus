use super::*;
use crate::bus::transport::TransportError;

struct NoNativeInput;
impl Transport for NoNativeInput {
    fn request(&mut self, _method: Method) -> Result<ResponseResult, TransportError> {
        panic!("Queued focus must reach the UI before any native command");
    }
}

fn fixture() -> (Worker, RoomId, AgentId, PathBuf) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("temp")
        .join(format!(
            "bus-focus-{}-{}",
            std::process::id(),
            crate::bus::io::now_ns()
        ));
    let mut worker = Worker::open(dir.clone(), Box::new(NoNativeInput)).unwrap();
    worker.dev_enabled = true;
    let room = worker.state.create_room("review").unwrap();
    let agent = worker
        .state
        .create_agent(
            room,
            "cursor1",
            Provider::Cursor,
            PathBuf::from(env!("CARGO_MANIFEST_DIR")),
            None,
        )
        .unwrap();
    worker
        .state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                pane_id: Some("w1:p2".into()),
                ..Default::default()
            },
        )
        .unwrap();
    (worker, room, agent, dir)
}

fn request(id: &str, method: &str, params: Value) -> ControlRequest {
    ControlRequest {
        id: id.into(),
        method: method.into(),
        params,
        capability: None,
    }
}

#[test]
fn dev_focus_queues_real_ui_event_without_typing_and_deduplicates() {
    let (mut worker, room, agent, dir) = fixture();
    let (events, receiver) = mpsc::channel();
    let call = request(
        "agent-focus",
        "agent.focus",
        json!({"agent":agent.0.to_string()}),
    );
    let response = worker.dev_response_with_events(&call, Some(&events));
    assert!(response.ok, "{response:?}");
    assert_eq!(
        response.result,
        json!({"stage":"queued", "agent_id":agent, "room_id":room})
    );
    assert!(
        matches!(receiver.try_recv().unwrap(), BusEvent::DevFocusRequested {
        room: selected, agent: Some(target)
    } if selected == room && target == agent)
    );
    assert_eq!(
        worker.dev_response_with_events(&call, Some(&events)).result,
        response.result
    );
    assert!(
        receiver.try_recv().is_err(),
        "Retry must not switch the user's view again"
    );
    let response = worker.dev_response_with_events(
        &request("room-focus", "room.focus", json!({"room":"review"})),
        Some(&events),
    );
    assert!(response.ok, "{response:?}");
    assert_eq!(response.result, json!({"stage":"queued", "room_id":room}));
    assert!(
        matches!(receiver.try_recv().unwrap(), BusEvent::DevFocusRequested {
        room: selected, agent: None
    } if selected == room)
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_focus_rejects_normal_mode_missing_target_and_disconnected_ui() {
    let (mut worker, _room, agent, dir) = fixture();
    let (events, receiver) = mpsc::channel();
    worker.dev_enabled = false;
    let response = worker.dev_response_with_events(
        &request("disabled", "agent.focus", json!({"agent":"cursor1"})),
        Some(&events),
    );
    assert_eq!(response.error.unwrap().code, "dev_disabled");
    worker.dev_enabled = true;
    let response = worker.dev_response_with_events(
        &request("missing", "room.focus", json!({"room":"missing"})),
        Some(&events),
    );
    assert!(!response.ok);
    assert!(receiver.try_recv().is_err());
    drop(receiver);
    let response = worker.dev_response_with_events(
        &request(
            "disconnected",
            "agent.focus",
            json!({"agent":agent.0.to_string()}),
        ),
        Some(&events),
    );
    assert!(!response.ok);
    assert!(response.error.unwrap().message.contains("UI"));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
