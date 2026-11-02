use super::*;
use crate::messaging::native::{Transport, TransportError};
use crate::protocol::api::schema::{Method, ResponseResult};

struct NoTransport;
impl Transport for NoTransport {
    fn request(&mut self, _: Method) -> Result<ResponseResult, TransportError> {
        panic!("restart quit must not call the native server");
    }
}

#[test]
fn non_dev_restart_quit_requires_the_published_plan_and_live_helper() {
    let _guard = crate::utils::config::test_config_env_lock().lock().unwrap();
    let root = crate::utils::test_temp::unique_temp_path("restart-authorization");
    let mut worker = Worker::open(root.clone(), Box::new(NoTransport)).unwrap();
    worker.dev_enabled = false;
    let live = crate::platform::process_instance(std::process::id()).unwrap();
    let owner = restart_record::LivePid {
        pid: live.pid,
        birth: live.birth,
    };
    let plan = HandoffPlan {
        id: "authorize-one".into(),
        root: root.clone(),
        old_binary: "/old".into(),
        new_binary: "/new".into(),
        old_sha: "a".repeat(40),
        new_sha: "b".repeat(40),
        who: "test".into(),
        tui: owner.clone(),
        session_id: None,
        explicit_data_dir: true,
        dev: false,
        yolo: true,
    };
    let (events, received) = mpsc::channel();
    let request = |id: &str, params| Request {
        id: id.into(),
        method: "bus.quit".into(),
        params,
    };
    let normal = worker
        .control_response_with_events(&request("normal", serde_json::json!({})), Some(&events));
    assert_eq!(normal.error.unwrap().code, "dev_tools_disabled");
    let params = serde_json::json!({"restart":{"plan":plan,"helper_pid":live.pid}});
    let missing =
        worker.control_response_with_events(&request("missing", params.clone()), Some(&events));
    assert!(!missing.ok);
    assert!(received.try_recv().is_err());
    assert!(take_restart_plan().is_none());
    restart_record::write_json(&restart_record::restart_lock_path(&root), &owner).unwrap();
    restart_record::write_json(&restart_record::handoff_path(&root), &plan).unwrap();
    std::fs::write(restart_record::handoff_path(&root), "not-json").unwrap();
    let malformed =
        worker.control_response_with_events(&request("malformed", params.clone()), Some(&events));
    assert!(!malformed.ok);
    assert!(malformed.error.unwrap().message.contains("expected"));
    assert!(received.try_recv().is_err());
    restart_record::write_json(&restart_record::handoff_path(&root), &plan).unwrap();
    let mut wrong = params.clone();
    wrong["restart"]["helper_pid"] = serde_json::json!(live.pid.saturating_add(1));
    assert!(
        !worker
            .control_response_with_events(&request("wrong-helper", wrong), Some(&events))
            .ok
    );
    let mut wrong = params.clone();
    wrong["restart"]["plan"]["new_sha"] = serde_json::json!("c".repeat(40));
    assert!(
        !worker
            .control_response_with_events(&request("wrong-plan", wrong), Some(&events))
            .ok
    );
    assert!(received.try_recv().is_err());
    let mut other_tui = plan.clone();
    other_tui.tui.pid = live.pid.saturating_add(1);
    restart_record::write_json(&restart_record::handoff_path(&root), &other_tui).unwrap();
    let wrong = serde_json::json!({"restart":{"plan":other_tui,"helper_pid":live.pid}});
    assert!(
        !worker
            .control_response_with_events(&request("wrong-tui", wrong), Some(&events))
            .ok
    );
    assert!(received.try_recv().is_err());
    restart_record::write_json(&restart_record::handoff_path(&root), &plan).unwrap();
    assert!(
        !worker
            .control_response_with_events(&request("no-ui", params.clone()), None)
            .ok
    );
    assert!(take_restart_plan().is_none());
    let (closed, receiver) = mpsc::channel();
    drop(receiver);
    assert!(
        !worker
            .control_response_with_events(&request("closed-ui", params.clone()), Some(&closed))
            .ok
    );
    assert!(take_restart_plan().is_none());
    let accepted = worker.control_response_with_events(&request("accepted", params), Some(&events));
    assert!(accepted.ok, "{accepted:?}");
    assert!(matches!(
        received.try_recv().unwrap(),
        BusEvent::DevQuitRequested
    ));
    assert_eq!(take_restart_plan().unwrap(), plan);
    assert!(take_restart_plan().is_none());
    drop(worker);
    std::fs::remove_dir_all(root).unwrap();
}
