use super::*;
use crate::messaging::native::TransportError;

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
            crate::messaging::storage::io::now_ns()
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
    let response = worker.control_response_with_events(&call, Some(&events));
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
        worker
            .control_response_with_events(&call, Some(&events))
            .result,
        response.result
    );
    assert!(
        receiver.try_recv().is_err(),
        "Retry must not switch the user's view again"
    );
    let response = worker.control_response_with_events(
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
    let response = worker.control_response_with_events(
        &request("disabled", "agent.focus", json!({"agent":"cursor1"})),
        Some(&events),
    );
    assert_eq!(response.error.unwrap().code, "dev_tools_disabled");
    worker.dev_enabled = true;
    let response = worker.control_response_with_events(
        &request("missing", "room.focus", json!({"room":"missing"})),
        Some(&events),
    );
    assert!(!response.ok);
    assert!(receiver.try_recv().is_err());
    drop(receiver);
    let response = worker.control_response_with_events(
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

#[test]
fn dev_quit_and_settings_reach_the_ui_and_state_reports_them() {
    let (mut worker, _room, agent, dir) = fixture();
    let (events, receiver) = mpsc::channel();
    let quit = request("quit", "bus.quit", json!({}));
    let response = worker.control_response_with_events(&quit, Some(&events));
    assert_eq!(response.result, json!({"stage":"queued"}));
    assert!(matches!(
        receiver.try_recv().unwrap(),
        BusEvent::DevQuitRequested
    ));
    worker.control_response_with_events(&quit, Some(&events));
    assert!(receiver.try_recv().is_err(), "a retry must not quit twice");
    let unavailable =
        worker.control_response_with_events(&request("quit-2", "bus.quit", json!({})), None);
    assert!(!unavailable.ok);

    let path = dir.join("settings").join("settings.json");
    let state = |worker: &mut Worker, id: &str| {
        worker
            .control_response_with_events(&request(id, "state", json!({})), None)
            .result
    };
    assert_eq!(state(&mut worker, "s0")["settings"], Value::Null);
    worker.settings_path = Some(path.clone());
    assert_eq!(
        state(&mut worker, "s1")["settings"]["color_blind_mode"],
        false
    );
    let on = worker.control_response_with_events(
        &request("cb-on", "settings.color_blind", json!({"on": true})),
        Some(&events),
    );
    assert!(on.ok, "{on:?}");
    assert!(matches!(
        receiver.try_recv().unwrap(),
        BusEvent::SettingsChanged(settings) if settings.color_blind_mode
    ));
    assert!(
        crate::messaging::prefs::settings::load(&path)
            .unwrap()
            .color_blind_mode
    );
    assert_eq!(
        state(&mut worker, "s2")["settings"]["color_blind_mode"],
        true
    );

    let details = worker.control_response_with_events(
        &request(
            "details",
            "agent.details",
            json!({"agent":"cursor1","on":true}),
        ),
        Some(&events),
    );
    assert!(details.ok, "{details:?}");
    assert!(worker.state.agent(agent).unwrap().details_disclosed);
    assert!(
        receiver.try_recv().is_err(),
        "details is a model command, not a UI event"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
