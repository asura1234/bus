use super::*;

#[tokio::test]
async fn startup_timeout_keeps_owned_terminal_readable_and_trust_chooser_addressable() {
    let (mut app, mut writes) = app_with_dialog(b"", None);
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let id = app.state.workspaces[0].terminal_id(pane).unwrap().clone();
    let now = std::time::Instant::now();
    let terminal = app.state.terminals.get_mut(&id).unwrap();
    terminal.begin_managed_agent(
        "reviewer".into(),
        AgentKind::Cursor,
        now,
        Duration::ZERO,
        Duration::from_secs(30),
    );
    terminal.set_detected_state(Some(AgentKind::Cursor), AgentState::Unknown);
    app.reconcile_managed_agents_at(now + Duration::from_secs(31));
    let before = app.agent_info(0, pane).unwrap();
    assert_eq!(before.name.as_deref(), Some("reviewer"));
    assert!(before.launch_pending && !before.interactive_ready);

    // A startup blocker may become recognizable only after the deadline.
    app.lookup_runtime_sender(0, pane).unwrap().test_process_pty_bytes(
        b"Workspace Trust Required\r\nDo you trust the contents of this directory?\r\n/private/tmp/scratch-project\r\n> [a] Trust this workspace\r\n  [q] Quit\r\nUse arrow keys to navigate, Enter to select, or press the key shown\r\n",
    );
    let response = app.handle_agent_read(
        "read".into(),
        crate::protocol::api::schema::AgentReadParams {
            target: "reviewer".into(),
            source: crate::protocol::api::schema::ReadSource::Visible,
            lines: None,
            format: crate::protocol::api::schema::ReadFormat::Text,
            strip_ansi: true,
        },
    );
    let read: SuccessResponse = serde_json::from_str(&response).expect(&response);
    let ResponseResult::PaneRead { read } = read.result else {
        panic!("{response}");
    };
    assert!(read
        .text
        .contains("Do you trust the contents of this directory?"));
    let observed = observe_dialog(&mut app);
    assert_eq!(observed.terminal_id, before.terminal_id);
    assert_eq!(observed.pane_id, before.pane_id);
    let dialog = observed.dialog.as_ref().expect("late trust chooser");
    assert!(chosen(&choose(&mut app, &observed, None, &dialog.digest, 1)).written);
    assert_eq!(writes.recv().await.unwrap().as_ref(), b"a");
    let terminal = app.state.terminals.get_mut(&id).unwrap();
    terminal.set_detected_state(Some(AgentKind::Cursor), AgentState::Idle);
    terminal.reconcile_managed_agent_at(now + Duration::from_secs(32), false);
    let recovered = app.agent_info(0, pane).unwrap();
    assert_eq!(recovered.name, before.name);
    assert_eq!(recovered.terminal_id, before.terminal_id);
    assert!(recovered.interactive_ready);
}
