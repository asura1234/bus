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
            selection_label: None,
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

#[tokio::test]
async fn universal_driver_select_ignores_quoted_text_outside_inverse_menu() {
    let (mut app, target, mut writes) = shell(b"> quoted context\r\n\x1b[7mYes\x1b[0m\r\nNo");
    let mut params = press(&target);
    params.select = Some("Yes".into());
    let result: Value =
        serde_json::from_str(&app.handle_agent_press("select".into(), params)).unwrap();
    assert_eq!(
        result["result"]["press"]["keys"],
        serde_json::json!(["enter"])
    );
    assert_eq!(writes.try_recv().unwrap().as_ref(), b"\r");
}

#[tokio::test]
async fn universal_driver_selection_redraw_retains_menu_context() {
    let (mut app, target, mut writes) = shell(b"> quoted context\r\n\x1b[7mYes\x1b[0m\r\nNo");
    let mut params = press(&target);
    params.select = Some("No".into());
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
        .test_process_pty_bytes(b"\x1b[2J\x1b[H> quoted context\r\nYes\r\n\x1b[7mNo\x1b[0m");
    let observed: Value = serde_json::from_str(&app.handle_agent_observe(
        "redraw".into(),
        AgentObserveParams {
            target: target.clone(),
            selection_label: Some("No".into()),
        },
    ))
    .unwrap();
    assert_eq!(observed["result"]["observation"]["focus"][1], "No");
    let final_press: Value =
        serde_json::from_str(&app.handle_agent_press("final".into(), params)).unwrap();
    assert_eq!(
        final_press["result"]["press"]["keys"],
        serde_json::json!(["enter"])
    );
    assert_eq!(writes.try_recv().unwrap().as_ref(), b"\r");
}

#[tokio::test]
async fn universal_driver_description_redraw_cannot_repeat_pending_navigation() {
    let (mut app, target, mut writes) = shell(
        "  1. Continue\r\n     note\r\n❯ 2. Continue\r\n     explanation\r\n  3. Done".as_bytes(),
    );
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
    app.lookup_runtime_sender(0, pane).unwrap().test_process_pty_bytes("\x1b[2J\x1b[H  1. Continue\r\n     Continue\r\n❯ 2. Continue\r\n     explanation\r\n  3. Done".as_bytes());
    let pending: Value =
        serde_json::from_str(&app.handle_agent_press("repeat".into(), params)).unwrap();
    assert_eq!(pending["result"]["press"]["keys"], serde_json::json!([]));
    assert_eq!(pending["result"]["press"]["selection_pending"], true);
    assert!(
        writes.try_recv().is_err(),
        "description text is not a new focused option"
    );
}

#[tokio::test]
async fn driver_agent_metadata_marks_an_incomplete_frame_unreadable() {
    let (mut app, _, mut writes) = shell(b"A coherent screen");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let id = app.state.workspaces[0].terminal_id(pane).unwrap().clone();
    app.state
        .terminals
        .get_mut(&id)
        .unwrap()
        .set_detected_state(
            Some(crate::agents::AgentKind::Claude),
            crate::agents::AgentState::Idle,
        );
    assert!(!app.agent_info(0, pane).unwrap().screen_detection_skipped);
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .test_process_pty_bytes(b"\x1b[?2026hpartial redraw");
    assert!(
        app.agent_info(0, pane).unwrap().screen_detection_skipped,
        "an incomplete driver read cannot attest that a blocker closed"
    );
    assert!(writes.try_recv().is_err());
}

#[tokio::test]
async fn driver_agent_metadata_keeps_pending_questions_out_of_working_episodes() {
    use crate::agents::{AgentKind, AgentState};
    let pending = concat!(
        "• Working (12s • esc to interrupt)\r\n",
        "• Queued follow-up inputs\r\n",
        "  ? 1 question\r\n",
        "    shift+left to answer\r\n\r\n",
        "› Ask Codex to do anything\r\n",
    );
    let foreground = concat!(
        "• Working (12s • esc to interrupt)\r\n",
        "What token should Bus use?\r\n",
        "Type your answer\r\n",
        "enter submit   ctrl+] skip   shift+→ main prompt\r\n",
    );
    for (state, screen, has_episode) in [
        (AgentState::Working, pending, false),
        (AgentState::Idle, pending, true),
        (AgentState::Working, foreground, true),
    ] {
        let (mut app, _, mut writes) = shell(screen.as_bytes());
        let pane = app.state.workspaces[0].tabs[0].root_pane;
        let id = app.state.workspaces[0].terminal_id(pane).unwrap().clone();
        app.state
            .terminals
            .get_mut(&id)
            .unwrap()
            .set_detected_state(Some(AgentKind::Codex), state);
        let info = app.agent_info(0, pane).unwrap();
        assert!(!info.screen_detection_skipped);
        assert_eq!(info.dialog_id.is_some(), has_episode, "{state:?}: {screen}");
        assert!(
            writes.try_recv().is_err(),
            "metadata observation is read-only"
        );
    }
}

#[tokio::test]
async fn cursor_monthly_usage_limit_screen_is_provider_error_metadata() {
    use crate::agents::{AgentKind, AgentState};
    let limited = concat!(
        "Error: You've reached your monthly usage limit\r\n",
        "Request higher limits to continue using Cursor\r\n",
        " > [Pasted text #1 +3 lines]\r\n",
        " / commands · @ files",
    );
    let ordinary = concat!(
        "Error: You've reached your monthly usage limit\r\n",
        "Request higher limits to continue using Cursor\r\n",
        " > Add a follow-up\r\n",
        " / commands · @ files",
    );
    for (screen, limited) in [(limited, true), (ordinary, false)] {
        let (mut app, _, mut writes) = shell(screen.as_bytes());
        let pane = app.state.workspaces[0].tabs[0].root_pane;
        let id = app.state.workspaces[0].terminal_id(pane).unwrap().clone();
        app.state
            .terminals
            .get_mut(&id)
            .unwrap()
            .set_detected_state(Some(AgentKind::Cursor), AgentState::Idle);
        let info = serde_json::to_value(app.agent_info(0, pane).unwrap()).unwrap();
        if limited {
            assert_eq!(
                info["provider_error"],
                "You've reached your monthly usage limit. Request higher limits to continue using Cursor (rate_limit)"
            );
            assert!(info["provider_quota_reset_at"].as_u64().is_some());
        } else {
            assert_eq!(info["provider_error"], Value::Null);
            assert_eq!(info["provider_quota_reset_at"], Value::Null);
        }
        assert!(
            writes.try_recv().is_err(),
            "reading a restored draft must not clear it or type over it"
        );
    }
}

#[tokio::test]
async fn cursor_quota_metadata_survives_warning_wrap_in_a_narrow_pane() {
    use crate::agents::{AgentKind, AgentState};
    let screen = concat!(
        "Error: You've reached your monthly usage limit\r\n",
        "Request higher limits to continue using Cursor\r\n",
        " > [Pasted text #1 +3 lines]\r\n",
        " / commands · @ files",
    );
    let (mut app, _, _) = shell(b"");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let (runtime, mut writes) =
        crate::terminal::TerminalRuntime::test_with_channel_capacity(40, 24, 16);
    runtime.test_process_pty_bytes(screen.as_bytes());
    app.state.insert_test_runtime(pane, runtime);
    let id = app.state.workspaces[0].terminal_id(pane).unwrap().clone();
    app.state
        .terminals
        .get_mut(&id)
        .unwrap()
        .set_detected_state(Some(AgentKind::Cursor), AgentState::Idle);
    let info = serde_json::to_value(app.agent_info(0, pane).unwrap()).unwrap();
    assert_eq!(
        info["provider_error"],
        "You've reached your monthly usage limit. Request higher limits to continue using Cursor (rate_limit)"
    );
    assert!(info["provider_quota_reset_at"].as_u64().is_some());
    assert!(
        writes.try_recv().is_err(),
        "quota observation must not modify the restored draft"
    );
}
