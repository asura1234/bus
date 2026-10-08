use super::*;
use crate::{
    api::schema::{ErrorResponse, SuccessResponse},
    config::Config,
    detect::{Agent, AgentState},
    workspace::Workspace,
};

fn app_with_test_workspace() -> (App, String) {
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(
        &Config::default(),
        crate::server::app::AppPolicy::TEST,
        None,
        api_rx,
        crate::server::api::EventHub::default(),
    );
    app.state.workspaces = vec![Workspace::test_new("metadata")];
    app.state.ensure_test_terminals();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    let public_pane_id = app.public_pane_id(0, pane_id).unwrap();
    (app, public_pane_id)
}

fn guarded_close_fixture() -> (
    App,
    crate::protocol::api::schema::PaneCloseIfIdentityParams,
    PaneId,
    PaneId,
) {
    let (mut app, public_pane_id) = app_with_test_workspace();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    let other = app.state.workspaces[0].test_split(ratatui::layout::Direction::Horizontal);
    app.state.ensure_test_terminals();
    let terminal_id = app.state.terminal_id_for_pane(0, pane_id).unwrap();
    let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
    terminal.begin_managed_agent(
        "bus-r1-a2".into(),
        Agent::Codex,
        std::time::Instant::now(),
        std::time::Duration::ZERO,
        std::time::Duration::from_secs(10),
    );
    terminal.set_detected_state(Some(Agent::Codex), AgentState::Working);
    let params = crate::protocol::api::schema::PaneCloseIfIdentityParams {
        pane_id: public_pane_id,
        expected_terminal_id: terminal_id.to_string(),
        expected_agent: "codex".into(),
        expected_managed_name: "bus-r1-a2".into(),
        expected_session_id: None,
    };
    (app, params, pane_id, other)
}

fn app_with_send_key_runtime(
    capacity: usize,
) -> (App, String, tokio::sync::mpsc::Receiver<bytes::Bytes>) {
    let (mut app, public_pane_id) = app_with_test_workspace();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    let (runtime, rx) =
        crate::terminal::TerminalRuntime::test_with_channel_capacity(80, 24, capacity);
    app.state.insert_test_runtime(pane_id, runtime);
    (app, public_pane_id, rx)
}

fn app_with_scrollback_runtime() -> (App, String, PaneId) {
    let (mut app, public_pane_id) = app_with_test_workspace();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    let lines = (0..20)
        .map(|line| format!("line {line:02}\n"))
        .collect::<String>();
    let runtime =
        crate::terminal::TerminalRuntime::test_with_scrollback_bytes(20, 5, 1000, lines.as_bytes());
    app.state.insert_test_runtime(pane_id, runtime);
    (app, public_pane_id, pane_id)
}

fn app_with_one_workspace() -> App {
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(
        &Config::default(),
        crate::server::app::AppPolicy::TEST,
        None,
        api_rx,
        crate::server::api::EventHub::default(),
    );
    app.state.workspaces = vec![Workspace::test_new("issue")];
    app.state.ensure_test_terminals();
    app
}

#[path = "layout_test.rs"]
mod layout;

#[path = "navigation_test.rs"]
mod navigation;

#[path = "copy_test.rs"]
mod copy;

#[path = "io_test.rs"]
mod io;

#[path = "close_test.rs"]
mod close;

#[test]
fn api_pane_report_agent_session_preserves_session_identity_and_detected_state() {
    let (mut app, public_pane_id) = app_with_test_workspace();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = app.state.terminal_id_for_pane(0, pane_id).unwrap();
    app.state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .set_detected_state(Some(Agent::Codex), AgentState::Working);

    let response = app.handle_api_request(crate::protocol::api::schema::Request {
        id: "session-report".into(),
        method: crate::protocol::api::schema::Method::PaneReportAgentSession(
            PaneReportAgentSessionParams {
                pane_id: public_pane_id,
                source: "herdr:codex".into(),
                agent: "  CODEX  ".into(),
                seq: Some(1),
                agent_session_id: Some("codex-session".into()),
                agent_session_path: None,
                session_start_source: Some(" startup ".into()),
            },
        ),
    });

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(success.id, "session-report");
    assert_eq!(success.result, ResponseResult::Ok {});
    let terminal = &app.state.terminals[&terminal_id];
    assert_eq!(terminal.state, AgentState::Working);
    let session = terminal.persisted_agent_session.as_ref().unwrap();
    assert_eq!(session.source, "herdr:codex");
    assert_eq!(session.agent, "codex");
    assert_eq!(session.session_ref.value, "codex-session");
    assert_eq!(
        session.session_ref.kind,
        crate::agent_resume::AgentSessionRefKind::Id
    );
}

#[test]
fn api_pane_report_agent_session_rejects_missing_panes_and_empty_agents() {
    let (mut app, public_pane_id) = app_with_test_workspace();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = app.state.terminal_id_for_pane(0, pane_id).unwrap();
    for (target, agent, expected_code) in [
        ("missing-pane".to_owned(), "codex", "pane_not_found"),
        (public_pane_id, " \t\n ", "invalid_agent"),
    ] {
        let response = app.handle_api_request(crate::protocol::api::schema::Request {
            id: "invalid-session-report".into(),
            method: crate::protocol::api::schema::Method::PaneReportAgentSession(
                PaneReportAgentSessionParams {
                    pane_id: target,
                    source: "herdr:codex".into(),
                    agent: agent.into(),
                    seq: Some(1),
                    agent_session_id: Some("codex-session".into()),
                    agent_session_path: None,
                    session_start_source: Some("startup".into()),
                },
            ),
        });
        let error: ErrorResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(error.id, "invalid-session-report");
        assert_eq!(error.error.code, expected_code);
        assert!(app.state.terminals[&terminal_id]
            .persisted_agent_session
            .is_none());
    }
}
