use super::*;
use crate::{
    agents::{AgentKind, AgentState},
    protocol::api::schema::{AgentStatus, SuccessResponse},
    server::{app::Mode, workspaces::Workspace},
    utils::config::Config,
};

fn app_with_agent() -> App {
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(
        &Config::default(),
        crate::server::app::AppPolicy::TEST,
        None,
        api_rx,
        crate::server::api::EventHub::default(),
    );
    app.state.workspaces = vec![Workspace::test_new("agent")];
    app.state.ensure_test_terminals();
    app.state.active = Some(0);
    app.state.selected = 0;
    app.state.mode = Mode::Terminal;
    app
}

fn start_deferred_agent_prompt(
    app: &mut App,
    id: &str,
    params: AgentPromptParams,
) -> std::sync::mpsc::Receiver<String> {
    let (respond_to, response_rx) = std::sync::mpsc::channel();
    assert!(app.handle_deferred_agent_api_request(
        crate::protocol::api::schema::Request {
            id: id.into(),
            method: crate::protocol::api::schema::Method::AgentPrompt(params),
        },
        respond_to,
    ));
    response_rx
}

fn run_deferred_agent_prompt(app: &mut App, id: &str, params: AgentPromptParams) -> String {
    start_deferred_agent_prompt(app, id, params)
        .recv_timeout(Duration::from_secs(1))
        .expect("agent prompt responds after submission")
}

/// A ready, idle Claude agent whose screen ends with `input` in its box.
fn claude_agent_with_input(
    input: &str,
) -> (
    App,
    crate::protocol::api::schema::AgentPromptIfIdleParams,
    tokio::sync::mpsc::Receiver<Bytes>,
) {
    let mut app = app_with_agent();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
        .attached_terminal_id
        .clone();
    let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
    let now = std::time::Instant::now();
    terminal.begin_managed_agent(
        "bus-r1-a2".into(),
        AgentKind::Claude,
        now,
        Duration::ZERO,
        Duration::from_secs(10),
    );
    terminal.set_detected_state(Some(AgentKind::Claude), AgentState::Idle);
    terminal.reconcile_managed_agent_at(now + Duration::from_secs(1), false);
    terminal.set_agent_session_ref_for_session_start(
        "bus".into(),
        "claude".into(),
        crate::agents::resume::catalog::AgentSessionRef::id("session"),
        Some(1),
        None,
    );
    let (runtime, writes) =
        crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(80, 24, 0, b"", 2);
    let rule = "─".repeat(40);
    runtime.test_process_pty_bytes(
        format!("\x1b[?2004h\x1b[2J\x1b[H\x1b[2m{rule}\x1b[0m\r\n{input}\r\n\x1b[2m{rule}\x1b[0m")
            .as_bytes(),
    );
    app.state.insert_test_runtime(pane_id, runtime);
    let info = app.agent_info(0, pane_id).unwrap();
    let params = crate::protocol::api::schema::AgentPromptIfIdleParams {
        target: info.pane_id.clone(),
        text: "from bus".into(),
        expected_terminal_id: info.terminal_id,
        expected_pane_id: info.pane_id,
        expected_agent: "claude".into(),
        expected_session_id: "session".into(),
        steer: false,
    };
    (app, params, writes)
}

const CLAUDE_BASH_DIALOG: &[u8] = b" Do you want to proceed?\r\n \xe2\x9d\xaf 1. Yes\r\n   2. Yes, and don't ask again for: curl *\r\n   3. No\r\n\r\n Esc to cancel \xc2\xb7 Tab to amend\r\n";

/// A named Claude agent showing `screen`; `session` is unset while launching.
fn app_with_dialog(
    screen: &[u8],
    session: Option<&str>,
) -> (App, tokio::sync::mpsc::Receiver<Bytes>) {
    let mut app = app_with_agent();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
        .attached_terminal_id
        .clone();
    let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
    terminal.set_agent_name("reviewer".into());
    terminal.set_detected_state(Some(AgentKind::Claude), AgentState::Blocked);
    if let Some(session) = session {
        terminal.set_agent_session_ref_for_session_start(
            "bus".into(),
            "claude".into(),
            crate::agents::resume::catalog::AgentSessionRef::id(session),
            Some(1),
            None,
        );
    }
    let (runtime, writes) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
    runtime.test_process_pty_bytes(screen);
    app.state.insert_test_runtime(pane_id, runtime);
    (app, writes)
}

fn observe_dialog(app: &mut App) -> AgentDialogObservation {
    let observed = app.handle_agent_dialog_observe(
        "observe".into(),
        crate::protocol::api::schema::AgentDialogObserveParams {
            target: "reviewer".into(),
            open_pending_question: false,
        },
    );
    let success: SuccessResponse = serde_json::from_str(&observed).unwrap();
    let ResponseResult::AgentDialog { observation } = success.result else {
        panic!("unexpected observation response");
    };
    observation
}

fn choose(
    app: &mut App,
    observation: &AgentDialogObservation,
    session: Option<&str>,
    digest: &str,
    option: u32,
) -> String {
    app.handle_agent_dialog_choose(
        "choose".into(),
        AgentDialogChooseParams {
            target: "reviewer".into(),
            expected_terminal_id: observation.terminal_id.clone(),
            expected_pane_id: observation.pane_id.clone(),
            expected_session_id: session.map(str::to_owned),
            expected_dialog_digest: digest.into(),
            option,
        },
    )
}

fn chosen(response: &str) -> AgentDialogChooseResult {
    let success: SuccessResponse = serde_json::from_str(response).unwrap();
    let ResponseResult::AgentDialogChosen { choice } = success.result else {
        panic!("unexpected choice response: {response}");
    };
    choice
}

const CODEX_TEXT_DIALOG: &[u8] = "• Queued follow-up inputs\r\n\r\nWhat token should Bus use?\r\n\r\nType your answer\r\n\r\nenter submit   ctrl+] skip   shift+→ main prompt\r\n".as_bytes();

fn answer_params(
    observation: &AgentDialogObservation,
    text: Option<&str>,
    skip: bool,
) -> AgentDialogAnswerParams {
    AgentDialogAnswerParams {
        target: "reviewer".into(),
        expected_terminal_id: observation.terminal_id.clone(),
        expected_pane_id: observation.pane_id.clone(),
        expected_session_id: observation.session_id.clone(),
        expected_dialog_digest: observation.dialog.as_ref().unwrap().digest.clone(),
        text: text.map(str::to_owned),
        skip,
    }
}

#[path = "prompt_test.rs"]
mod prompt;

#[path = "dialog_test.rs"]
mod dialog;

#[tokio::test]
async fn agent_send_keys_validates_every_key_before_writing() {
    let mut app = app_with_agent();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
        .attached_terminal_id
        .clone();
    let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
    terminal.set_agent_name("reviewer".into());
    terminal.set_detected_state(Some(AgentKind::Pi), AgentState::Idle);
    let (runtime, mut rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
    app.state.insert_test_runtime(pane_id, runtime);

    let rejected = app.handle_agent_send_keys(
        "req-invalid".into(),
        AgentSendKeysParams {
            target: "reviewer".into(),
            keys: vec!["enter".into(), "not-a-key".into()],
        },
    );
    let error: crate::protocol::api::schema::ErrorResponse =
        serde_json::from_str(&rejected).unwrap();
    assert_eq!(error.error.code, "invalid_key");
    assert!(rx.try_recv().is_err());

    let sent = app.handle_agent_send_keys(
        "req-valid".into(),
        AgentSendKeysParams {
            target: "reviewer".into(),
            keys: vec!["up".into(), "enter".into()],
        },
    );
    let success: SuccessResponse = serde_json::from_str(&sent).unwrap();
    assert!(matches!(success.result, ResponseResult::Ok {}));
    assert_eq!(rx.try_recv().unwrap(), Bytes::from_static(b"\x1b[A\r"));
    assert!(rx.try_recv().is_err());
}

#[test]
fn agent_focus_marks_already_focused_done_agent_seen() {
    let mut app = app_with_agent();
    app.state.outer_terminal_focus = Some(false);

    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
        .attached_terminal_id
        .clone();
    app.state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .set_detected_state(Some(AgentKind::Pi), AgentState::Idle);
    app.state.workspaces[0].tabs[0]
        .panes
        .get_mut(&pane_id)
        .unwrap()
        .seen = false;
    app.state.workspaces[0].tabs[0].layout.focus_pane(pane_id);

    let response = app.handle_agent_focus(
        "req".into(),
        AgentTarget {
            target: app.public_pane_id(0, pane_id).unwrap(),
        },
    );

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::AgentInfo { agent } = success.result else {
        panic!("expected agent info response");
    };
    assert_eq!(agent.agent_status, AgentStatus::Idle);
}

#[test]
fn agent_rename_does_not_replace_the_pane_label() {
    let mut app = app_with_agent();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
        .attached_terminal_id
        .clone();
    let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
    terminal.set_manual_label("shell-pane".into());
    terminal.set_detected_state(Some(AgentKind::Pi), AgentState::Idle);
    let target = app.public_pane_id(0, pane_id).unwrap();

    for name in [Some("reviewer".to_string()), None] {
        let response = app.handle_agent_rename(
            "req".into(),
            AgentRenameParams {
                target: target.clone(),
                name,
            },
        );
        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert!(matches!(success.result, ResponseResult::AgentInfo { .. }));
        assert_eq!(
            app.state.terminals[&terminal_id].manual_label.as_deref(),
            Some("shell-pane")
        );
    }
}
