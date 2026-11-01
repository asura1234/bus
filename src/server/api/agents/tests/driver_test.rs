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
    assert!(observed.get("error").is_none(), "{observed}");
    assert!(observed["result"]["observation"]["text"]
        .as_str()
        .unwrap()
        .contains("[Y/n]"));
    let mut params = press(&target);
    params.keys = vec!["n".into()];
    let result: Value =
        serde_json::from_str(&app.handle_agent_press("answer".into(), params)).unwrap();
    assert!(result.get("error").is_none(), "{result}");
    assert_eq!(writes.recv().await.unwrap().as_ref(), b"n");
    assert!(writes.try_recv().is_err());
}

#[tokio::test]
async fn universal_driver_select_waits_for_outstanding_navigation_across_calls() {
    let (mut app, target, mut writes) =
        shell("❯ 1. Install update now\r\n  2. Continue with current version".as_bytes());
    let mut params = press(&target);
    params.select = Some("Continue with current version".into());
    let result: Value =
        serde_json::from_str(&app.handle_agent_press("select".into(), params.clone())).unwrap();
    assert!(result.get("error").is_none(), "{result}");
    assert_eq!(result["result"]["press"]["selection_pending"], true);
    assert_eq!(writes.recv().await.unwrap().as_ref(), b"\x1b[B");
    assert!(writes.try_recv().is_err());
    let repeated: Value =
        serde_json::from_str(&app.handle_agent_press("continue".into(), params.clone())).unwrap();
    assert_eq!(repeated["result"]["press"]["keys"], serde_json::json!([]));
    assert_eq!(repeated["result"]["press"]["selection_pending"], true);
    assert!(
        writes.try_recv().is_err(),
        "the first arrow has not applied yet"
    );
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .test_process_pty_bytes(
            "\x1b[2J\x1b[H  1. Install update now\r\n❯ 2. Continue with current version".as_bytes(),
        );
    let confirmed: Value =
        serde_json::from_str(&app.handle_agent_press("confirm".into(), params)).unwrap();
    assert_eq!(confirmed["result"]["press"]["selection_pending"], false);
    assert_eq!(writes.recv().await.unwrap().as_ref(), b"\r");
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
    assert!(result.get("error").is_some(), "{result}");
    params.keys.clear();
    params.expected_terminal_id = Some("old-terminal".into());
    let result: Value =
        serde_json::from_str(&app.handle_agent_press("old-terminal".into(), params.clone()))
            .unwrap();
    assert!(result.get("error").is_some(), "{result}");
    assert!(writes.try_recv().is_err());
    params.expected_terminal_id = None;
    let result: Value =
        serde_json::from_str(&app.handle_agent_press("text".into(), params)).unwrap();
    assert!(result.get("error").is_none(), "{result}");
    assert_eq!(writes.recv().await.unwrap().as_ref(), b"tui-driver-demo");
    assert!(writes.try_recv().is_err(), "text must not imply Enter");
}

#[tokio::test]
async fn universal_driver_submits_bracketed_text_with_a_separate_enter() {
    let (mut app, target, mut writes) = shell(b"\x1b[?2004hProject name: ");
    let mut params = press(&target);
    params.text = Some("tui-driver-demo".into());
    params.submit = true;
    let result: Value =
        serde_json::from_str(&app.handle_agent_press("submit".into(), params)).unwrap();
    assert!(result.get("error").is_none(), "{result}");
    assert_eq!(
        writes.recv().await.unwrap().as_ref(),
        b"\x1b[200~tui-driver-demo\x1b[201~"
    );
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(2), writes.recv())
            .await
            .unwrap()
            .unwrap()
            .as_ref(),
        b"\r"
    );
}

#[tokio::test]
async fn universal_driver_select_after_explicit_navigation_uses_current_focus() {
    let initial = "❯ 1. Install update now\r\n  2. Continue with current version";
    let focused = "\x1b[2J\x1b[H  1. Install update now\r\n❯ 2. Continue with current version";
    let (mut app, target, mut writes) = shell(initial.as_bytes());
    let mut select = press(&target);
    select.select = Some("Continue with current version".into());
    let first: Value =
        serde_json::from_str(&app.handle_agent_press("select".into(), select.clone())).unwrap();
    assert!(first.get("error").is_none(), "{first}");
    assert_eq!(writes.try_recv().unwrap().as_ref(), b"\x1b[B");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .test_process_pty_bytes(focused.as_bytes());
    let mut up = press(&target);
    up.keys = vec!["up".into()];
    let moved: Value = serde_json::from_str(&app.handle_agent_press("up".into(), up)).unwrap();
    assert!(moved.get("error").is_none(), "{moved}");
    assert_eq!(writes.try_recv().unwrap().as_ref(), b"\x1b[A");
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .test_process_pty_bytes(format!("\x1b[2J\x1b[H{initial}").as_bytes());
    let repeated: Value =
        serde_json::from_str(&app.handle_agent_press("select-again".into(), select)).unwrap();
    assert_eq!(repeated["result"]["press"]["keys"], serde_json::json!(["down"]),
        "the previous move applied before explicit Up; a fresh selection must not remain blocked by its old focus");
    assert_eq!(writes.try_recv().unwrap().as_ref(), b"\x1b[B");
}

#[tokio::test]
async fn universal_driver_traverses_equal_intermediate_labels() {
    let (mut app, target, mut writes) = shell("❯Continue\r\n Continue\r\n Done".as_bytes());
    let mut params = press(&target);
    params.select = Some("Done".into());
    let first: Value =
        serde_json::from_str(&app.handle_agent_press("first".into(), params.clone())).unwrap();
    assert_eq!(
        first["result"]["press"]["keys"],
        serde_json::json!(["down"])
    );
    assert_eq!(writes.try_recv().unwrap().as_ref(), b"\x1b[B");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .test_process_pty_bytes("\x1b[2J\x1b[H Continue\r\n❯Continue\r\n Done".as_bytes());
    let second: Value =
        serde_json::from_str(&app.handle_agent_press("second".into(), params)).unwrap();
    assert_eq!(
        second["result"]["press"]["keys"],
        serde_json::json!(["down"])
    );
    assert_eq!(writes.try_recv().unwrap().as_ref(), b"\x1b[B");
}

#[tokio::test]
async fn universal_driver_failed_explicit_input_preserves_outstanding_selection() {
    let (mut app, target, mut writes) = shell("❯Yes\r\n No".as_bytes());
    let mut select = press(&target);
    select.select = Some("No".into());
    let first: Value =
        serde_json::from_str(&app.handle_agent_press("first".into(), select.clone())).unwrap();
    assert_eq!(
        first["result"]["press"]["keys"],
        serde_json::json!(["down"])
    );
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    for _ in 0..15 {
        app.lookup_runtime_sender(0, pane)
            .unwrap()
            .try_send_developer_bytes(Bytes::from_static(b"x"))
            .unwrap();
    }
    let mut up = press(&target);
    up.keys.push("up".into());
    let failed: Value = serde_json::from_str(&app.handle_agent_press("full".into(), up)).unwrap();
    assert_eq!(failed["error"]["code"], "terminal_write_failed");
    while writes.try_recv().is_ok() {}
    let repeated: Value =
        serde_json::from_str(&app.handle_agent_press("repeat".into(), select)).unwrap();
    assert_eq!(repeated["result"]["press"]["keys"], serde_json::json!([]));
    assert_eq!(repeated["result"]["press"]["selection_pending"], true);
    assert!(writes.try_recv().is_err());
}

#[tokio::test]
async fn universal_driver_enqueue_failures_log_destination_and_cause_without_answers() {
    for full in [false, true] {
        let (mut app, target, mut writes) = shell(b"Answer: ");
        if full {
            let pane = app.state.workspaces[0].tabs[0].root_pane;
            for _ in 0..16 {
                app.lookup_runtime_sender(0, pane)
                    .unwrap()
                    .try_send_developer_bytes(Bytes::from_static(b"x"))
                    .unwrap();
            }
        } else {
            writes.close();
        }
        let mut params = press(&target);
        params.text = Some("PRIVATE-ANSWER-NEVER-LOG".into());
        let capture = crate::utils::logging::test_capture::Capture::default();
        capture.run_filtered(crate::utils::logging::DEV_FILTER, || {
            tracing::callsite::rebuild_interest_cache();
            let result: Value =
                serde_json::from_str(&app.handle_agent_press("enqueue-proof".into(), params))
                    .unwrap();
            assert_eq!(result["error"]["code"], "terminal_write_failed");
        });
        let log = capture.text();
        for expected in [
            "bus.terminal.driver.enqueue_failed",
            "enqueue-proof",
            "terminal_id=",
            "pane_id=",
            "input_mode=\"text\"",
            if full {
                "no available capacity"
            } else {
                "closed"
            },
        ] {
            assert!(log.contains(expected), "missing {expected}: {log}");
        }
        assert!(!log.contains("PRIVATE-ANSWER-NEVER-LOG"), "{log}");
    }
}

#[tokio::test]
async fn universal_driver_empty_text_keeps_outstanding_navigation() {
    let (mut app, target, mut writes) = shell("❯Yes\r\n No".as_bytes());
    let mut select = press(&target);
    select.select = Some("No".into());
    let first: Value =
        serde_json::from_str(&app.handle_agent_press("first".into(), select.clone())).unwrap();
    assert_eq!(
        first["result"]["press"]["keys"],
        serde_json::json!(["down"])
    );
    assert_eq!(writes.try_recv().unwrap().as_ref(), b"\x1b[B");
    let mut empty = press(&target);
    empty.text = Some(String::new());
    let noop: Value = serde_json::from_str(&app.handle_agent_press("noop".into(), empty)).unwrap();
    assert!(noop.get("error").is_none(), "{noop}");
    assert!(writes.try_recv().is_err());
    let resumed: Value =
        serde_json::from_str(&app.handle_agent_press("resume".into(), select)).unwrap();
    assert_eq!(resumed["result"]["press"]["keys"], serde_json::json!([]));
    assert!(
        writes.try_recv().is_err(),
        "empty text must not release an unapplied arrow"
    );
}
