use super::*;
use crate::agents::{AgentKind, AgentState};

#[cfg(unix)]
fn init_repo(path: &std::path::Path) {
    let status = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(path)
        .status()
        .unwrap();
    assert!(status.success(), "git init failed for {}", path.display());
}

fn app_with_overlay(
    workspace: crate::server::workspaces::Workspace,
    overlay_pane: crate::utils::ids::PaneId,
    previous_focus: crate::utils::ids::PaneId,
    previous_zoomed: bool,
) -> App {
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(
        &crate::utils::config::Config::default(),
        crate::server::app::AppPolicy::TEST,
        None,
        api_rx,
        crate::server::api::EventHub::default(),
    );
    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    app.state.active = Some(0);
    app.state.mode = Mode::Terminal;
    app.overlay_panes.insert(
        overlay_pane,
        OverlayPaneState {
            ws_idx: 0,
            tab_idx: 0,
            previous_focus,
            previous_zoomed,
            temp_files: Vec::new(),
        },
    );
    app
}

#[path = "events_test.rs"]
mod events;

#[test]
fn client_window_title_api_reports_no_foreground_client_in_app_mode() {
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(
        &crate::utils::config::Config::default(),
        crate::server::app::AppPolicy::TEST,
        None,
        api_rx,
        crate::server::api::EventHub::default(),
    );

    let set = app.handle_api_request(crate::protocol::api::schema::Request {
        id: "title_set".into(),
        method: crate::protocol::api::schema::Method::ClientWindowTitleSet(
            crate::protocol::api::schema::ClientWindowTitleSetParams {
                title: "plugin review".into(),
            },
        ),
    });
    let set: serde_json::Value = serde_json::from_str(&set).unwrap();
    assert_eq!(set["result"]["type"], "client_window_title");
    assert_eq!(set["result"]["changed"], false);
    assert_eq!(set["result"]["reason"], "no_foreground_client");

    let clear = app.handle_api_request(crate::protocol::api::schema::Request {
        id: "title_clear".into(),
        method: crate::protocol::api::schema::Method::ClientWindowTitleClear(
            crate::protocol::api::schema::EmptyParams::default(),
        ),
    });
    let clear: serde_json::Value = serde_json::from_str(&clear).unwrap();
    assert_eq!(clear["result"]["type"], "client_window_title");
    assert_eq!(clear["result"]["reason"], "no_foreground_client");
}

#[cfg(unix)]
#[tokio::test]
async fn herdr_toast_context_uses_live_root_runtime_cwd_label() {
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(
        &crate::utils::config::Config::default(),
        crate::server::app::AppPolicy::TEST,
        None,
        api_rx,
        crate::server::api::EventHub::default(),
    );

    let mut workspace = crate::server::workspaces::Workspace::test_new("stale");
    workspace.custom_name = None;
    let root = workspace.tabs[0].root_pane;
    let terminal_id = workspace.terminal_id(root).cloned().unwrap();
    let temp_root = std::env::temp_dir().join(format!(
        "bus-toast-context-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let stale_cwd = temp_root.join("__bus_original__");
    let live_cwd = temp_root.join("__bus_projects__");
    std::fs::create_dir_all(&stale_cwd).unwrap();
    std::fs::create_dir_all(&live_cwd).unwrap();
    init_repo(&stale_cwd);
    init_repo(&live_cwd);

    workspace.identity_cwd = stale_cwd.clone();
    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    app.state.terminals.get_mut(&terminal_id).unwrap().cwd = stale_cwd;
    app.state.active = None;
    app.state.selected = 0;
    app.state.mode = Mode::Terminal;
    app.state.toast_config.delivery = crate::utils::config::ToastDelivery::Bus;
    app.state.toast_config.delay_seconds = 0;

    let (events, _) = tokio::sync::mpsc::channel(4);
    let runtime = crate::terminal::TerminalRuntime::spawn(
        root,
        24,
        80,
        live_cwd.clone(),
        0,
        crate::utils::theme::color::TerminalTheme::default(),
        None,
        crate::terminal::runtime::spawn::PaneShellConfig::new(
            "/bin/sh",
            crate::utils::config::ShellModeConfig::NonLogin,
        ),
        &crate::terminal::runtime::spawn::PaneLaunchEnv::default(),
        events,
        std::sync::Arc::new(tokio::sync::Notify::new()),
        std::sync::Arc::new(crate::utils::render::signal::RenderSignal::new()),
    )
    .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while runtime.cwd() != Some(live_cwd.clone()) && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    app.terminal_runtimes.insert(terminal_id, runtime);

    app.handle_internal_event(TerminalEvent::StateChanged {
        pane_id: root,
        agent: Some(AgentKind::Codex),
        state: AgentState::Working,
        visible_blocker: false,
        process_exited: false,
        exited_process: None,
        observed_at: std::time::Instant::now(),
    });
    app.handle_internal_event(TerminalEvent::StateChanged {
        pane_id: root,
        agent: Some(AgentKind::Codex),
        state: AgentState::Idle,
        visible_blocker: false,
        process_exited: false,
        exited_process: None,
        observed_at: std::time::Instant::now(),
    });

    assert_eq!(
        app.state.toast.as_ref().map(|toast| toast.context.as_str()),
        Some("__bus_projects__ · 1")
    );

    for (_, runtime) in app.terminal_runtimes.drain() {
        runtime.shutdown();
    }
    let _ = std::fs::remove_dir_all(temp_root);
}

#[cfg(unix)]
#[tokio::test]
async fn delayed_herdr_toast_context_uses_live_root_runtime_cwd_label() {
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(
        &crate::utils::config::Config::default(),
        crate::server::app::AppPolicy::TEST,
        None,
        api_rx,
        crate::server::api::EventHub::default(),
    );

    let mut workspace = crate::server::workspaces::Workspace::test_new("stale");
    workspace.custom_name = None;
    let root = workspace.tabs[0].root_pane;
    let terminal_id = workspace.terminal_id(root).cloned().unwrap();
    let temp_root = std::env::temp_dir().join(format!(
        "bus-delayed-toast-context-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let stale_cwd = temp_root.join("__bus_original__");
    let live_cwd = temp_root.join("__bus_projects__");
    std::fs::create_dir_all(&stale_cwd).unwrap();
    std::fs::create_dir_all(&live_cwd).unwrap();
    init_repo(&stale_cwd);
    init_repo(&live_cwd);

    workspace.identity_cwd = stale_cwd.clone();
    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    app.state.terminals.get_mut(&terminal_id).unwrap().cwd = stale_cwd;
    app.state.active = None;
    app.state.selected = 0;
    app.state.mode = Mode::Terminal;
    app.state.toast_config.delivery = crate::utils::config::ToastDelivery::Bus;
    app.state.toast_config.delay_seconds = 1;

    let (events, _) = tokio::sync::mpsc::channel(4);
    let runtime = crate::terminal::TerminalRuntime::spawn(
        root,
        24,
        80,
        live_cwd.clone(),
        0,
        crate::utils::theme::color::TerminalTheme::default(),
        None,
        crate::terminal::runtime::spawn::PaneShellConfig::new(
            "/bin/sh",
            crate::utils::config::ShellModeConfig::NonLogin,
        ),
        &crate::terminal::runtime::spawn::PaneLaunchEnv::default(),
        events,
        std::sync::Arc::new(tokio::sync::Notify::new()),
        std::sync::Arc::new(crate::utils::render::signal::RenderSignal::new()),
    )
    .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while runtime.cwd() != Some(live_cwd.clone()) && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    app.terminal_runtimes.insert(terminal_id, runtime);

    app.handle_internal_event(TerminalEvent::StateChanged {
        pane_id: root,
        agent: Some(AgentKind::Codex),
        state: AgentState::Working,
        visible_blocker: false,
        process_exited: false,
        exited_process: None,
        observed_at: std::time::Instant::now(),
    });
    app.handle_internal_event(TerminalEvent::StateChanged {
        pane_id: root,
        agent: Some(AgentKind::Codex),
        state: AgentState::Idle,
        visible_blocker: false,
        process_exited: false,
        exited_process: None,
        observed_at: std::time::Instant::now(),
    });

    let notification_deadline = app
        .state
        .next_pending_agent_notification_deadline()
        .expect("pending notification deadline");
    let mut deliveries = app
        .state
        .drain_due_agent_notifications(notification_deadline);
    app.refresh_agent_notification_delivery_contexts(&mut deliveries);
    assert_eq!(
        deliveries
            .first()
            .and_then(|delivery| delivery.toast.as_ref())
            .map(|toast| toast.context.as_str()),
        Some("__bus_projects__ · 1")
    );

    for (_, runtime) in app.terminal_runtimes.drain() {
        runtime.shutdown();
    }
    let _ = std::fs::remove_dir_all(temp_root);
}

#[test]
fn overlay_exit_preserves_focus_changed_before_exit() {
    let mut workspace = crate::server::workspaces::Workspace::test_new("overlay");
    let previous_focus = workspace.tabs[0].root_pane;
    let overlay_pane = workspace.test_split(ratatui::layout::Direction::Horizontal);
    workspace.tabs[0].zoomed = true;
    let new_tab = workspace.test_add_tab(Some("new"));
    workspace.switch_tab(new_tab);
    let mut app = app_with_overlay(workspace, overlay_pane, previous_focus, true);

    app.handle_internal_event(TerminalEvent::PaneDied {
        pane_id: overlay_pane,
        exit_reason: crate::platform::ChildExitReason::Exited,
    });

    let overlay_tab = &app.state.workspaces[0].tabs[0];
    assert_eq!(app.state.workspaces[0].active_tab, new_tab);
    assert_eq!(overlay_tab.layout.focused(), previous_focus);
    assert!(overlay_tab.zoomed);
    assert!(app.overlay_panes.is_empty());
}

#[test]
fn overlay_exit_preserves_same_tab_focus_changed_before_exit() {
    let mut workspace = crate::server::workspaces::Workspace::test_new("overlay");
    let previous_focus = workspace.tabs[0].root_pane;
    let overlay_pane = workspace.test_split(ratatui::layout::Direction::Horizontal);
    workspace.tabs[0].layout.focus_pane(previous_focus);
    workspace.tabs[0].zoomed = true;
    let mut app = app_with_overlay(workspace, overlay_pane, previous_focus, false);

    app.handle_internal_event(TerminalEvent::PaneDied {
        pane_id: overlay_pane,
        exit_reason: crate::platform::ChildExitReason::Exited,
    });

    let tab = &app.state.workspaces[0].tabs[0];
    assert_eq!(app.state.workspaces[0].active_tab, 0);
    assert_eq!(tab.layout.focused(), previous_focus);
    assert!(tab.zoomed);
    assert!(app.overlay_panes.is_empty());
}

#[test]
fn overlay_exit_restores_previous_focus_when_overlay_still_focused() {
    let mut workspace = crate::server::workspaces::Workspace::test_new("overlay");
    let previous_focus = workspace.tabs[0].root_pane;
    let overlay_pane = workspace.test_split(ratatui::layout::Direction::Horizontal);
    workspace.tabs[0].zoomed = true;
    let mut app = app_with_overlay(workspace, overlay_pane, previous_focus, false);

    app.handle_internal_event(TerminalEvent::PaneDied {
        pane_id: overlay_pane,
        exit_reason: crate::platform::ChildExitReason::Exited,
    });

    let tab = &app.state.workspaces[0].tabs[0];
    assert_eq!(app.state.workspaces[0].active_tab, 0);
    assert_eq!(tab.layout.focused(), previous_focus);
    assert!(!tab.zoomed);
    assert!(app.overlay_panes.is_empty());
}

#[tokio::test]
async fn pane_died_respawns_shell_and_clears_restored_agent_session() {
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(
        &crate::utils::config::Config::default(),
        crate::server::app::AppPolicy::TEST,
        None,
        api_rx,
        crate::server::api::EventHub::default(),
    );
    let workspace = crate::server::workspaces::Workspace::test_new("restored");
    app.state.default_shell = test_support::exiting_test_command().into();
    let pane_id = workspace.tabs[0].root_pane;
    let terminal_id = workspace.terminal_id(pane_id).cloned().unwrap();
    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    let terminal = app
        .state
        .terminals
        .get_mut(&terminal_id)
        .expect("test terminal should exist");
    terminal.respawn_shell_on_exit = true;
    terminal.set_agent_name("codex".into());
    terminal.set_persisted_agent_session(crate::agents::resume::catalog::PersistedAgentSession {
        source: "herdr:codex".into(),
        agent: "codex".into(),
        session_ref: crate::agents::resume::catalog::AgentSessionRef::id("codex-session")
            .expect("test session id should be valid"),
    });

    app.handle_internal_event(TerminalEvent::PaneDied {
        pane_id,
        exit_reason: crate::platform::ChildExitReason::Exited,
    });

    assert!(
        app.find_pane(pane_id).is_some(),
        "respawnable agent pane should stay attached after the agent process exits"
    );
    let terminal = app
        .state
        .terminals
        .get(&terminal_id)
        .expect("terminal should survive respawn");
    assert!(!terminal.respawn_shell_on_exit);
    assert!(terminal.persisted_agent_session.is_none());
    assert!(terminal.agent_name.is_none());

    for (_, runtime) in app.terminal_runtimes.drain() {
        runtime.shutdown();
    }
}

#[cfg(windows)]
#[test]
fn windows_powershell_exit_after_agent_process_exit_respawns_shell() {
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(
        &crate::utils::config::Config::default(),
        crate::server::app::AppPolicy::TEST,
        None,
        api_rx,
        crate::server::api::EventHub::default(),
    );
    let workspace = crate::server::workspaces::Workspace::test_new("powershell");
    let pane_id = workspace.tabs[0].root_pane;
    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    app.state.default_shell = "powershell.exe".into();
    app.state.shell_mode = crate::utils::config::ShellModeConfig::NonLogin;

    app.handle_internal_event(TerminalEvent::StateChanged {
        pane_id,
        agent: Some(crate::agents::AgentKind::OpenCode),
        state: AgentState::Idle,
        visible_blocker: false,
        process_exited: true,
        exited_process: None,
        observed_at: std::time::Instant::now(),
    });

    assert_eq!(
        app.runtime_exit_action(pane_id),
        RuntimeExitAction::RespawnShell
    );
}

#[cfg(windows)]
#[test]
fn windows_powershell_exit_without_recent_agent_process_exit_closes_pane() {
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(
        &crate::utils::config::Config::default(),
        crate::server::app::AppPolicy::TEST,
        None,
        api_rx,
        crate::server::api::EventHub::default(),
    );
    let workspace = crate::server::workspaces::Workspace::test_new("powershell");
    let pane_id = workspace.tabs[0].root_pane;
    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    app.state.default_shell = "powershell.exe".into();
    app.state.shell_mode = crate::utils::config::ShellModeConfig::NonLogin;

    assert_eq!(
        app.runtime_exit_action(pane_id),
        RuntimeExitAction::ClosePane
    );
}
