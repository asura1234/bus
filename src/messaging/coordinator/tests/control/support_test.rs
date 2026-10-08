use super::*;
use crate::bus::transport::TransportError;
use serde_json::json;

#[path = "agents_test.rs"]
mod agents_tests;
#[path = "dialogs_test.rs"]
mod dialogs_tests;
#[path = "inspect_test.rs"]
mod inspect_tests;
#[path = "messages_test.rs"]
mod messages_tests;
#[path = "rooms_test.rs"]
mod rooms_tests;

struct NoTransport;

impl Transport for NoTransport {
    fn request(&mut self, _method: Method) -> Result<ResponseResult, TransportError> {
        panic!("Domain-only command unexpectedly reached native transport")
    }
}

static NEXT_FIXTURE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn fixture() -> (Worker, RoomId, AgentId, PathBuf) {
    // Parallel tests can share a nanosecond; the counter keeps each coordinator lock unique.
    let dir = std::env::temp_dir().join(format!(
        "bus-control-domain-{}-{}-{}",
        std::process::id(),
        crate::messaging::storage::io::now_ns(),
        NEXT_FIXTURE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let mut worker = Worker::open(dir.clone(), Box::new(NoTransport)).unwrap();
    worker.dev_enabled = true;
    let room = worker.state.create_room("test").unwrap();
    let agent = worker
        .state
        .create_agent(room, "codex1", Provider::Codex, dir.clone(), None)
        .unwrap();
    (worker, room, agent, dir)
}

fn call(worker: &mut Worker, id: &str, method: &str, params: serde_json::Value) -> Response {
    worker.dev_response(&ControlRequest {
        id: id.into(),
        method: method.into(),
        params,
    })
}

fn owned_pane_info() -> schema::PaneInfo {
    serde_json::from_value(json!({
        "pane_id": "w1:p2",
        "terminal_id": "term_internal",
        "workspace_id": "w1",
        "tab_id": "t1",
        "focused": false,
        "agent_status": "working",
        "revision": 3
    }))
    .unwrap()
}

fn owned_agent_info(pane_id: &str, name: &str, session: Option<&str>) -> schema::AgentInfo {
    let mut value = json!({
        "terminal_id": "term_internal",
        "pane_id": pane_id,
        "name": name,
        "agent_status": "idle",
        "workspace_id": "w1",
        "tab_id": "t1",
        "focused": false,
        "revision": 1
    });
    if let Some(session) = session {
        value["agent_session"] = json!({
            "source": "herdr:codex",
            "agent": "codex",
            "kind": "id",
            "value": session
        });
    }
    serde_json::from_value(value).unwrap()
}

fn error_message(response: &Response) -> String {
    response
        .error
        .as_ref()
        .map(|error| error.message.clone())
        .unwrap_or_default()
}

/// Rejects the agent's tab, so an add stops after Bus prepared its folder.
struct NoTabs;

impl Transport for NoTabs {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        assert!(matches!(method, Method::TabCreate(_)), "{method:?}");
        Err(TransportError {
            code: None,
            message: "no tabs in tests".into(),
            definitely_rejected: true,
        })
    }
}

fn launch_spool(worker: &Worker, agent: AgentId) -> PathBuf {
    let launch = worker
        .state
        .agent(agent)
        .unwrap()
        .runtime_identity
        .launch_id
        .clone();
    worker.data_dir.join("callbacks").join(launch.unwrap())
}

fn messages_to(worker: &Worker, agent: AgentId) -> Vec<String> {
    worker
        .state
        .requests()
        .filter(|r| r.agent_id == agent)
        .map(|r| r.prompt.text.clone())
        .collect()
}
