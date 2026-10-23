use super::*;
use crate::protocol::api::schema::AgentDialogChoiceAction as Action;

const CAPTURE: &str =
    include_str!("../../../../../tests/fixtures/provider-ui/claude-extra-thought.txt");

fn screen(selected: u32) -> String {
    if selected == 2 {
        CAPTURE
            .replace("❯ 1.", "  1.")
            .replace("  2.", "❯ 2.")
            .replace('\n', "\r\n")
    } else {
        CAPTURE.replace('\n', "\r\n")
    }
}

fn control(
    app: &mut App,
    observed: &AgentDialogObservation,
    option: u32,
    action: Action,
) -> String {
    app.handle_agent_dialog_choose(
        "auto-ui".into(),
        AgentDialogChooseParams {
            target: "reviewer".into(),
            expected_terminal_id: observed.terminal_id.clone(),
            expected_pane_id: observed.pane_id.clone(),
            expected_session_id: observed.session_id.clone(),
            expected_dialog_digest: observed.dialog.as_ref().unwrap().digest.clone(),
            option,
            action,
        },
    )
}

#[tokio::test]
async fn auto_ui_focus_sends_all_arrows_immediately_and_never_queues_enter() {
    for (option, keys) in [(2, b"\x1b[B".as_slice()), (3, b"\x1b[B\x1b[B".as_slice())] {
        let (mut app, mut writes) =
            app_with_dialog_sized(screen(1).as_bytes(), Some("session"), 134, 45);
        let observed = observe_dialog(&mut app);
        let result = chosen(&control(&mut app, &observed, option, Action::Focus));
        assert!(result.written, "{result:?}");
        assert_eq!(writes.try_recv().unwrap().as_ref(), keys);
        assert!(
            tokio::time::timeout(Duration::from_millis(180), writes.recv())
                .await
                .is_err(),
            "no delayed confirmation or navigation"
        );
    }
}

#[tokio::test]
async fn auto_ui_confirm_requires_a_fresh_selected_target() {
    let (mut app, mut writes) =
        app_with_dialog_sized(screen(1).as_bytes(), Some("session"), 134, 45);
    let initial = observe_dialog(&mut app);
    assert!(!chosen(&control(&mut app, &initial, 2, Action::Confirm)).written);
    assert!(writes.try_recv().is_err());
    assert!(chosen(&control(&mut app, &initial, 2, Action::Focus)).written);
    assert_eq!(writes.try_recv().unwrap(), Bytes::from_static(b"\x1b[B"));
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .test_process_pty_bytes(format!("\x1b[2J\x1b[H{}", screen(2)).as_bytes());
    let focused = observe_dialog(&mut app);
    assert_eq!(
        initial.dialog.as_ref().unwrap().id,
        focused.dialog.as_ref().unwrap().id
    );
    assert!(!chosen(&control(&mut app, &initial, 2, Action::Confirm)).written);
    assert!(writes.try_recv().is_err());
    assert!(chosen(&control(&mut app, &focused, 2, Action::Confirm)).written);
    assert_eq!(writes.try_recv().unwrap(), Bytes::from_static(b"\r"));
    assert!(writes.try_recv().is_err());
}

#[tokio::test]
async fn auto_ui_confirm_refuses_a_permission_replacing_the_focused_prompt() {
    let (mut app, mut writes) =
        app_with_dialog_sized(screen(2).as_bytes(), Some("session"), 134, 45);
    let observed = observe_dialog(&mut app);
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    app.lookup_runtime_sender(0, pane).unwrap().test_process_pty_bytes("\x1b[2J\x1b[HDo you want to proceed?\r\n  1. Yes\r\n❯ 2. Yes, and don't ask again for: curl *\r\n  3. No\r\n\r\nEsc to cancel · Tab to amend\r\n".as_bytes());
    let permission = observe_dialog(&mut app)
        .dialog
        .expect("replacement is a real native permission dialog");
    assert_eq!(permission.options.len(), 3);
    assert!(permission.options[1].selected);
    let result = chosen(&control(&mut app, &observed, 2, Action::Confirm));
    assert!(!result.written);
    assert_eq!(result.reason.as_deref(), Some("stale_or_changed_dialog"));
    assert!(
        writes.try_recv().is_err(),
        "never approve the replacement permission"
    );
}

#[tokio::test]
async fn auto_ui_both_phases_share_the_bounded_frame_guard_and_send_once() {
    for (selected, action, key) in [
        (1, Action::Focus, b"\x1b[B".as_slice()),
        (2, Action::Confirm, b"\r".as_slice()),
    ] {
        let original = screen(selected);
        let (mut app, mut writes) =
            app_with_dialog_sized(original.as_bytes(), Some("session"), 134, 45);
        let observed = observe_dialog(&mut app);
        let pane = app.state.workspaces[0].tabs[0].root_pane;
        app.lookup_runtime_sender(0, pane)
            .unwrap()
            .test_process_pty_bytes(b"\x1b[?2026h\x1b[2J\x1b[H");
        let result: serde_json::Value =
            serde_json::from_str(&control(&mut app, &observed, 2, action)).unwrap();
        assert_eq!(result["error"]["code"], "dialog_redrawing");
        assert!(writes.try_recv().is_err());
        let writer = app
            .lookup_runtime_sender(0, pane)
            .unwrap()
            .test_pty_writer();
        let observed_wait = app
            .lookup_runtime_sender(0, pane)
            .unwrap()
            .test_observe_dialog_redraw(Duration::from_secs(2));
        let painter = std::thread::spawn(move || {
            observed_wait
                .recv_timeout(Duration::from_secs(10))
                .expect("guard observes the unfinished frame before repaint");
            writer(format!("{original}\x1b[?2026l").as_bytes());
        });
        let response = control(&mut app, &observed, 2, action);
        painter.join().unwrap();
        assert!(chosen(&response).written, "{response}");
        assert_eq!(writes.try_recv().unwrap().as_ref(), key);
        assert!(
            writes.try_recv().is_err(),
            "frame retry must not duplicate a phase"
        );
    }
}
