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
