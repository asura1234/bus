use crate::protocol::api::schema::{AgentObserveParams, AgentPressParams};
use crate::server::{
    api::EventHub,
    app::{App, AppPolicy},
    workspaces::Workspace,
};
use crate::utils::config::Config;
use bytes::Bytes;
use serde_json::Value;

fn shell(screen: &[u8]) -> (App, String, tokio::sync::mpsc::Receiver<Bytes>) {
    let (_, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(
        &Config::default(),
        AppPolicy::TEST,
        None,
        api_rx,
        EventHub::default(),
    );
    app.state.workspaces = vec![Workspace::test_new("shell")];
    app.state.ensure_test_terminals();
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let (runtime, writes) =
        crate::terminal::TerminalRuntime::test_with_channel_capacity(80, 24, 16);
    runtime.test_process_pty_bytes(screen);
    app.state.insert_test_runtime(pane, runtime);
    let target = app.public_pane_id(0, pane).unwrap();
    (app, target, writes)
}

fn press(target: &str) -> AgentPressParams {
    AgentPressParams {
        target: target.into(),
        expected_terminal_id: None,
        select: None,
        text: None,
        keys: vec![],
        submit: false,
    }
}

#[tokio::test]
async fn universal_driver_answers_shell_prompt_without_agent_metadata() {
    let (mut app, target, mut writes) = shell(b"[oh-my-zsh] Would you like to update? [Y/n] ");
    let observed: Value = serde_json::from_str(&app.handle_agent_observe(
        "read".into(),
        AgentObserveParams {
            target: target.clone(),
        },
    ))
    .unwrap();
    assert_eq!(observed["ok"], true);
    assert!(observed["result"]["observation"]["text"]
        .as_str()
        .unwrap()
        .contains("[Y/n]"));
    let mut params = press(&target);
    params.keys = vec!["n".into()];
    let result: Value =
        serde_json::from_str(&app.handle_agent_press("answer".into(), params)).unwrap();
    assert_eq!(result["ok"], true);
    assert_eq!(writes.recv().await.unwrap().as_ref(), b"n");
    assert!(writes.try_recv().is_err());
}

#[tokio::test]
async fn universal_driver_select_never_enters_the_initial_update_row() {
    let (mut app, target, mut writes) =
        shell("❯ 1. Install update now\r\n  2. Continue with current version".as_bytes());
    let mut params = press(&target);
    params.select = Some("Continue with current version".into());
    let result: Value =
        serde_json::from_str(&app.handle_agent_press("select".into(), params)).unwrap();
    assert_eq!(result["ok"], true);
    assert_eq!(result["result"]["press"]["selection_pending"], true);
    assert_eq!(writes.recv().await.unwrap().as_ref(), b"\x1b[B");
    assert!(writes.try_recv().is_err());
}

#[tokio::test]
async fn universal_driver_validates_keys_and_terminal_before_typing_text() {
    let (mut app, target, mut writes) = shell(b"Project name: ");
    let mut params = press(&target);
    params.text = Some("tui-driver-demo".into());
    params.keys = vec!["not-a-key".into()];
    let result: Value =
        serde_json::from_str(&app.handle_agent_press("bad-key".into(), params.clone())).unwrap();
    assert_eq!(result["ok"], false);
    params.keys.clear();
    params.expected_terminal_id = Some("old-terminal".into());
    let result: Value =
        serde_json::from_str(&app.handle_agent_press("old-terminal".into(), params.clone()))
            .unwrap();
    assert_eq!(result["ok"], false);
    assert!(writes.try_recv().is_err());
    params.expected_terminal_id = None;
    let result: Value =
        serde_json::from_str(&app.handle_agent_press("text".into(), params)).unwrap();
    assert_eq!(result["ok"], true);
    assert_eq!(writes.recv().await.unwrap().as_ref(), b"tui-driver-demo");
    assert!(writes.try_recv().is_err(), "text must not imply Enter");
}
