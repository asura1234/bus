use super::{
    api_session_kind, native_resume_is_empty_for_test, shell_command_from_argv,
    terminal_resume_identity_for_test, test_support,
};
#[cfg(unix)]
use super::{App, Rect};

#[test]
fn terminal_resume_facts_are_an_owned_snapshot_of_only_saved_identity() {
    let mut terminal = crate::terminal::TerminalState::new(
        crate::utils::ids::TerminalId::alloc(),
        std::env::temp_dir(),
    );
    let session = crate::agents::resume::catalog::PersistedAgentSession {
        source: "herdr:codex".into(),
        agent: "codex".into(),
        session_ref: crate::agents::resume::catalog::AgentSessionRef::id("saved-session").unwrap(),
    };
    terminal.restore_managed_agent("bus-r1-a2".into(), crate::agents::AgentKind::Codex);
    terminal.set_persisted_agent_session(session.clone());
    let (agent_name, managed_agent, saved_session) = terminal_resume_identity_for_test(&terminal);
    terminal.clear_agent_runtime_identity_after_respawn();
    assert_eq!(agent_name.as_deref(), Some("bus-r1-a2"));
    assert_eq!(managed_agent.as_deref(), Some("codex"));
    assert_eq!(saved_session, Some(session));
}

#[test]
fn native_resume_entry_keeps_foreign_server_context_out_of_capture_validation() {
    let root = crate::utils::test_temp::unique_temp_path("bus-resume-foreign-context");
    let mut terminal =
        crate::terminal::TerminalState::new(crate::utils::ids::TerminalId::alloc(), root.clone());
    terminal.restore_managed_agent("bus-r1-a2".into(), crate::agents::AgentKind::Codex);
    let plan = crate::agents::resume::catalog::AgentResumePlan {
        agent: "codex".into(),
        argv: vec!["invalid-saved-plan".into()],
        dedupe_key: "invalid".into(),
    };
    for (data_root, session_name) in [(None, None), (Some(root.as_path()), Some("nested"))] {
        let _env = test_support::ProcessEnvironment::enter(data_root, session_name);
        assert!(native_resume_is_empty_for_test(&terminal, &plan, &root).unwrap());
    }
}

#[test]
fn api_session_kind_keeps_domain_id_and_path_serialized_spellings() {
    for kind in [
        crate::agents::resume::catalog::AgentSessionRefKind::Id,
        crate::agents::resume::catalog::AgentSessionRefKind::Path,
    ] {
        assert_eq!(
            serde_json::to_value(api_session_kind(kind)).unwrap(),
            serde_json::to_value(kind).unwrap()
        );
    }
}

#[cfg(unix)]
fn test_app() -> App {
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    App::new(
        &crate::utils::config::Config::default(),
        crate::server::app::AppPolicy::TEST,
        None,
        api_rx,
        crate::server::api::EventHub::default(),
    )
}

#[cfg(unix)]
fn long_running_test_argv() -> Vec<String> {
    vec!["/bin/sh".into(), "-c".into(), "sleep 5".into()]
}

#[cfg(unix)]
fn marker_resume_test_argv() -> Vec<String> {
    vec![
        "/bin/sh".into(),
        "-c".into(),
        "printf '%s' 'restored agent: shell quoted | marker'; sleep 5".into(),
    ]
}

#[cfg(unix)]
#[tokio::test]
async fn invalid_bus_resume_context_does_not_launch_or_retry_and_keeps_session() {
    let root = crate::utils::test_temp::unique_temp_path("bus-resume-missing-context-test");
    let _env = test_support::ProcessEnvironment::enter(Some(&root), Some("bus"));
    let mut app = test_app();
    let workspace = crate::server::workspaces::Workspace::test_new("restored-bus");
    let terminal_id = workspace
        .terminal_id(workspace.tabs[0].root_pane)
        .cloned()
        .unwrap();
    app.state.view.pane_infos = workspace.tabs[0].layout.panes(Rect::new(0, 0, 100, 30));
    app.state.view.terminal_area = Rect::new(0, 0, 100, 30);
    app.state.workspaces = vec![workspace];
    app.state.active = Some(0);
    app.state.ensure_test_terminals();
    let session = crate::agents::resume::catalog::PersistedAgentSession {
        source: "herdr:codex".into(),
        agent: "codex".into(),
        session_ref: crate::agents::resume::catalog::AgentSessionRef::id("missing-bus-session")
            .unwrap(),
    };
    let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
    terminal.restore_managed_agent("bus-r999-a999".into(), crate::agents::AgentKind::Codex);
    terminal.set_persisted_agent_session(session.clone());
    terminal.pending_agent_resume_plan = Some(crate::agents::resume::catalog::AgentResumePlan {
        agent: "codex".into(),
        argv: marker_resume_test_argv(),
        dedupe_key: "test-untrusted-plan".into(),
    });

    assert!(app.start_pending_agent_resumes(true));
    assert!(
        app.terminal_runtimes.get(&terminal_id).is_none(),
        "an owned Bus terminal must not resume without validated capture context"
    );
    assert!(
        !app.has_pending_agent_resumes(),
        "failed context must not trigger file I/O on subsequent layout ticks"
    );
    let terminal = &app.state.terminals[&terminal_id];
    assert_eq!(terminal.state, crate::agents::AgentState::Unknown);
    assert_eq!(terminal.persisted_agent_session, Some(session));
    assert_eq!(terminal.agent_name.as_deref(), Some("bus-r999-a999"));
    assert!(!app.start_pending_agent_resumes(true));
}

#[cfg(unix)]
#[tokio::test]
async fn pending_agent_resume_waits_for_host_theme_before_launch() {
    let mut app = test_app();
    let workspace = crate::server::workspaces::Workspace::test_new("restored");
    let pane_id = workspace.tabs[0].root_pane;
    let terminal_id = workspace.terminal_id(pane_id).cloned().unwrap();
    let pane_infos = workspace.tabs[0]
        .layout
        .panes(ratatui::layout::Rect::new(0, 0, 100, 30));
    app.state.workspaces = vec![workspace];
    app.state.active = Some(0);
    app.state.ensure_test_terminals();
    app.state.view.terminal_area = ratatui::layout::Rect::new(0, 0, 100, 30);
    app.state.view.pane_infos = pane_infos;
    let terminal = app
        .state
        .terminals
        .get_mut(&terminal_id)
        .expect("test terminal should exist");
    terminal.pending_agent_resume_plan = Some(crate::agents::resume::catalog::AgentResumePlan {
        agent: "codex".into(),
        argv: marker_resume_test_argv(),
        dedupe_key: "herdr:codex\0codex\0Id\0codex-session".into(),
    });

    assert!(!app.start_pending_agent_resumes(false));
    assert!(app.terminal_runtimes.get(&terminal_id).is_none());

    app.state.host_terminal_theme = crate::utils::theme::color::TerminalTheme {
        foreground: Some(crate::utils::theme::color::RgbColor {
            r: 220,
            g: 220,
            b: 220,
        }),
        background: Some(crate::utils::theme::color::RgbColor {
            r: 20,
            g: 20,
            b: 20,
        }),
        ..Default::default()
    };

    assert!(app.start_pending_agent_resumes(false));
    assert!(app.terminal_runtimes.get(&terminal_id).is_some());
    let terminal = app
        .state
        .terminals
        .get(&terminal_id)
        .expect("terminal should survive launch");
    assert!(terminal.pending_agent_resume_plan.is_none());
    assert!(!terminal.respawn_shell_on_exit);

    let runtime = app
        .terminal_runtimes
        .get(&terminal_id)
        .expect("pending resume should leave a shell runtime");
    let marker = "restored agent: shell quoted | marker";
    for _ in 0..20 {
        if runtime
            .snapshot_history()
            .is_some_and(|text| text.contains(marker))
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(
        runtime
            .snapshot_history()
            .expect("runtime should expose terminal history")
            .contains(marker),
        "deferred restore should inject the resume argv into the restored shell"
    );

    for (_, runtime) in app.terminal_runtimes.drain() {
        runtime.shutdown();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn pending_agent_resume_can_launch_after_theme_wait_expires() {
    let mut app = test_app();
    let workspace = crate::server::workspaces::Workspace::test_new("restored");
    let pane_id = workspace.tabs[0].root_pane;
    let terminal_id = workspace.terminal_id(pane_id).cloned().unwrap();
    app.state.view.pane_infos = workspace.tabs[0]
        .layout
        .panes(ratatui::layout::Rect::new(0, 0, 100, 30));
    app.state.view.terminal_area = ratatui::layout::Rect::new(0, 0, 100, 30);
    app.state.workspaces = vec![workspace];
    app.state.active = Some(0);
    app.state.ensure_test_terminals();
    app.state
        .terminals
        .get_mut(&terminal_id)
        .expect("test terminal should exist")
        .pending_agent_resume_plan = Some(crate::agents::resume::catalog::AgentResumePlan {
        agent: "codex".into(),
        argv: long_running_test_argv(),
        dedupe_key: "herdr:codex\0codex\0Id\0codex-session".into(),
    });

    app.sync_pending_agent_resume_deadline(std::time::Instant::now());
    assert!(!app.start_pending_agent_resumes(false));
    assert!(app.start_pending_agent_resumes(true));
    assert!(app.terminal_runtimes.get(&terminal_id).is_some());

    for (_, runtime) in app.terminal_runtimes.drain() {
        runtime.shutdown();
    }
}

#[cfg(not(windows))]
#[tokio::test]
async fn pending_agent_resume_launches_hidden_panes_with_current_terminal_area() {
    let mut app = test_app();
    let active_workspace = crate::server::workspaces::Workspace::test_new("active");
    let active_pane = active_workspace.tabs[0].root_pane;
    let active_terminal = active_workspace.terminal_id(active_pane).cloned().unwrap();
    let hidden_workspace = crate::server::workspaces::Workspace::test_new("hidden");
    let hidden_pane = hidden_workspace.tabs[0].root_pane;
    let hidden_terminal = hidden_workspace.terminal_id(hidden_pane).cloned().unwrap();
    app.state.view.pane_infos = active_workspace.tabs[0]
        .layout
        .panes(ratatui::layout::Rect::new(0, 0, 100, 30));
    app.state.view.terminal_area = ratatui::layout::Rect::new(0, 0, 100, 30);
    app.state.workspaces = vec![active_workspace, hidden_workspace];
    app.state.active = Some(0);
    app.state.ensure_test_terminals();
    app.state.host_terminal_theme = crate::utils::theme::color::TerminalTheme {
        foreground: Some(crate::utils::theme::color::RgbColor {
            r: 220,
            g: 220,
            b: 220,
        }),
        background: Some(crate::utils::theme::color::RgbColor {
            r: 20,
            g: 20,
            b: 20,
        }),
        ..Default::default()
    };
    for terminal_id in [&active_terminal, &hidden_terminal] {
        app.state
            .terminals
            .get_mut(terminal_id)
            .expect("test terminal should exist")
            .pending_agent_resume_plan = Some(crate::agents::resume::catalog::AgentResumePlan {
            agent: "codex".into(),
            argv: long_running_test_argv(),
            dedupe_key: format!("herdr:codex\0codex\0Id\0{terminal_id}"),
        });
    }
    app.pending_agent_resume_deadline =
        Some(std::time::Instant::now() - std::time::Duration::from_millis(1));

    assert!(app.start_pending_agent_resumes(false));
    assert!(app.terminal_runtimes.get(&active_terminal).is_some());
    assert!(app.terminal_runtimes.get(&hidden_terminal).is_some());
    assert!(
        app.pending_agent_resume_deadline.is_none(),
        "launched pending resumes should clear the wakeup deadline"
    );

    for (_, runtime) in app.terminal_runtimes.drain() {
        runtime.shutdown();
    }
}

#[cfg(not(windows))]
#[tokio::test]
async fn pending_agent_resume_launches_inactive_tab_panes_with_current_terminal_area() {
    let mut app = test_app();
    let mut workspace = crate::server::workspaces::Workspace::test_new("tabs");
    let active_pane = workspace.tabs[0].root_pane;
    let inactive_tab = workspace.test_add_tab(Some("agents"));
    let inactive_pane = workspace.tabs[inactive_tab].root_pane;
    let inactive_terminal = workspace.tabs[inactive_tab]
        .terminal_id(inactive_pane)
        .cloned()
        .unwrap();
    app.state.view.pane_infos = workspace.tabs[0]
        .layout
        .panes(ratatui::layout::Rect::new(0, 0, 100, 30));
    app.state.view.terminal_area = ratatui::layout::Rect::new(0, 0, 100, 30);
    app.state.workspaces = vec![workspace];
    app.state.active = Some(0);
    app.state.ensure_test_terminals();
    assert!(app
        .state
        .workspaces
        .first()
        .and_then(|ws| ws.tabs[0].terminal_id(active_pane))
        .is_some());
    app.state.host_terminal_theme = crate::utils::theme::color::TerminalTheme {
        foreground: Some(crate::utils::theme::color::RgbColor {
            r: 220,
            g: 220,
            b: 220,
        }),
        background: Some(crate::utils::theme::color::RgbColor {
            r: 20,
            g: 20,
            b: 20,
        }),
        ..Default::default()
    };
    app.state
        .terminals
        .get_mut(&inactive_terminal)
        .expect("inactive tab terminal should exist")
        .pending_agent_resume_plan = Some(crate::agents::resume::catalog::AgentResumePlan {
        agent: "codex".into(),
        argv: long_running_test_argv(),
        dedupe_key: "herdr:codex\0codex\0Id\0inactive-tab-session".into(),
    });

    assert!(app.start_pending_agent_resumes(false));
    assert!(app.terminal_runtimes.get(&inactive_terminal).is_some());
    assert!(
        app.state
            .terminals
            .get(&inactive_terminal)
            .expect("inactive tab terminal should still exist")
            .pending_agent_resume_plan
            .is_none(),
        "inactive tab restored panes should not wait for tab focus"
    );

    for (_, runtime) in app.terminal_runtimes.drain() {
        runtime.shutdown();
    }
}

#[cfg(not(windows))]
#[tokio::test]
async fn pending_agent_resume_launches_zoom_hidden_active_tab_panes() {
    let mut app = test_app();
    let mut workspace = crate::server::workspaces::Workspace::test_new("zoomed");
    let hidden_pane = workspace.tabs[0].root_pane;
    let visible_pane = workspace.test_split(ratatui::layout::Direction::Horizontal);
    workspace.tabs[0].zoomed = true;
    let hidden_terminal = workspace.terminal_id(hidden_pane).cloned().unwrap();
    app.state.view.pane_infos = vec![crate::server::workspaces::layout::PaneInfo {
        id: visible_pane,
        rect: ratatui::layout::Rect::new(0, 0, 100, 30),
        inner_rect: ratatui::layout::Rect::new(1, 1, 98, 28),
        scrollbar_rect: None,
        borders: ratatui::widgets::Borders::ALL,
        is_focused: true,
    }];
    app.state.view.terminal_area = ratatui::layout::Rect::new(0, 0, 100, 30);
    app.state.workspaces = vec![workspace];
    app.state.active = Some(0);
    app.state.ensure_test_terminals();
    app.state.host_terminal_theme = crate::utils::theme::color::TerminalTheme {
        foreground: Some(crate::utils::theme::color::RgbColor {
            r: 220,
            g: 220,
            b: 220,
        }),
        background: Some(crate::utils::theme::color::RgbColor {
            r: 20,
            g: 20,
            b: 20,
        }),
        ..Default::default()
    };
    app.state
        .terminals
        .get_mut(&hidden_terminal)
        .expect("hidden zoom pane terminal should exist")
        .pending_agent_resume_plan = Some(crate::agents::resume::catalog::AgentResumePlan {
        agent: "codex".into(),
        argv: long_running_test_argv(),
        dedupe_key: "herdr:codex\0codex\0Id\0zoom-hidden-session".into(),
    });

    assert!(app.start_pending_agent_resumes(false));
    assert!(app.terminal_runtimes.get(&hidden_terminal).is_some());
    assert!(
        app.state
            .terminals
            .get(&hidden_terminal)
            .expect("hidden zoom pane terminal should still exist")
            .pending_agent_resume_plan
            .is_none(),
        "zoom-hidden restored panes should not wait for pane focus"
    );

    for (_, runtime) in app.terminal_runtimes.drain() {
        runtime.shutdown();
    }
}

#[cfg(not(windows))]
#[tokio::test]
async fn pending_agent_resume_uses_current_terminal_area_for_background_panes() {
    let mut app = test_app();
    let previous_workspace = crate::server::workspaces::Workspace::test_new("previous");
    let previous_pane = previous_workspace.tabs[0].root_pane;
    let previous_terminal = previous_workspace
        .terminal_id(previous_pane)
        .cloned()
        .unwrap();
    let current_workspace = crate::server::workspaces::Workspace::test_new("current");
    app.state.view.pane_infos = previous_workspace.tabs[0]
        .layout
        .panes(ratatui::layout::Rect::new(0, 0, 100, 30));
    app.state.view.terminal_area = ratatui::layout::Rect::new(0, 0, 80, 24);
    app.state.workspaces = vec![previous_workspace, current_workspace];
    app.state.active = Some(1);
    app.state.ensure_test_terminals();
    app.state.host_terminal_theme = crate::utils::theme::color::TerminalTheme {
        foreground: Some(crate::utils::theme::color::RgbColor {
            r: 220,
            g: 220,
            b: 220,
        }),
        background: Some(crate::utils::theme::color::RgbColor {
            r: 20,
            g: 20,
            b: 20,
        }),
        ..Default::default()
    };
    app.state
        .terminals
        .get_mut(&previous_terminal)
        .expect("test terminal should exist")
        .pending_agent_resume_plan = Some(crate::agents::resume::catalog::AgentResumePlan {
        agent: "codex".into(),
        argv: long_running_test_argv(),
        dedupe_key: "herdr:codex\0codex\0Id\0codex-session".into(),
    });

    app.sync_pending_agent_resume_deadline(std::time::Instant::now());
    assert!(app.pending_agent_resume_deadline.is_some());
    assert!(app.start_pending_agent_resumes(false));
    assert!(app.terminal_runtimes.get(&previous_terminal).is_some());
    assert!(
        app.state
            .terminals
            .get(&previous_terminal)
            .expect("previous terminal should still exist")
            .pending_agent_resume_plan
            .is_none(),
        "background restored panes should not wait for focus once terminal area is known"
    );

    for (_, runtime) in app.terminal_runtimes.drain() {
        runtime.shutdown();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn pending_agent_resume_launches_with_inner_rect_size() {
    let mut app = test_app();
    let mut workspace = crate::server::workspaces::Workspace::test_new("split");
    let pane_id = workspace.test_split(ratatui::layout::Direction::Horizontal);
    let terminal_id = workspace.terminal_id(pane_id).cloned().unwrap();
    app.state.view.pane_infos = vec![crate::server::workspaces::layout::PaneInfo {
        id: pane_id,
        rect: ratatui::layout::Rect::new(0, 0, 100, 30),
        inner_rect: ratatui::layout::Rect::new(1, 1, 98, 28),
        scrollbar_rect: None,
        borders: ratatui::widgets::Borders::ALL,
        is_focused: true,
    }];
    app.state.view.terminal_area = ratatui::layout::Rect::new(0, 0, 100, 30);
    app.state.workspaces = vec![workspace];
    app.state.active = Some(0);
    app.state.ensure_test_terminals();
    app.state.host_terminal_theme = crate::utils::theme::color::TerminalTheme {
        foreground: Some(crate::utils::theme::color::RgbColor {
            r: 220,
            g: 220,
            b: 220,
        }),
        background: Some(crate::utils::theme::color::RgbColor {
            r: 20,
            g: 20,
            b: 20,
        }),
        ..Default::default()
    };
    app.state
        .terminals
        .get_mut(&terminal_id)
        .expect("test terminal should exist")
        .pending_agent_resume_plan = Some(crate::agents::resume::catalog::AgentResumePlan {
        agent: "codex".into(),
        argv: long_running_test_argv(),
        dedupe_key: "herdr:codex\0codex\0Id\0codex-session".into(),
    });

    assert!(app.start_pending_agent_resumes(false));
    assert_eq!(
        app.terminal_runtimes
            .get(&terminal_id)
            .expect("pending resume should launch")
            .current_size(),
        (28, 98)
    );

    for (_, runtime) in app.terminal_runtimes.drain() {
        runtime.shutdown();
    }
}

#[test]
fn shell_command_from_argv_quotes_resume_arguments() {
    let argv = vec![
        "claude".to_string(),
        "--resume".to_string(),
        "session with ' quote".to_string(),
    ];

    assert_eq!(
        shell_command_from_argv(&argv),
        "claude --resume 'session with '\\'' quote'"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn pending_agent_resume_shell_spawn_failure_keeps_saved_conversation() {
    let _env = test_support::ProcessEnvironment::enter(None, None);
    let mut app = test_app();
    let workspace = crate::server::workspaces::Workspace::test_new("failed-resume-shell");
    let pane_id = workspace.tabs[0].root_pane;
    let terminal_id = workspace.terminal_id(pane_id).cloned().unwrap();
    app.state.view.pane_infos = workspace.tabs[0].layout.panes(Rect::new(0, 0, 100, 30));
    app.state.view.terminal_area = Rect::new(0, 0, 100, 30);
    app.state.workspaces = vec![workspace];
    app.state.active = Some(0);
    app.state.ensure_test_terminals();
    let missing_shell = std::env::temp_dir().join(format!(
        "bus-missing-resume-shell-{}-{}",
        std::process::id(),
        crate::utils::time::now_ns()
    ));
    assert!(!missing_shell.exists());
    app.state.default_shell = missing_shell.to_string_lossy().into_owned();
    app.state.shell_mode = crate::utils::config::ShellModeConfig::Login;
    let session = crate::agents::resume::catalog::PersistedAgentSession {
        source: "herdr:codex".into(),
        agent: "codex".into(),
        session_ref: crate::agents::resume::catalog::AgentSessionRef::id("recoverable-session")
            .unwrap(),
    };
    let plan =
        crate::agents::resume::catalog::plan(&session.source, &session.agent, &session.session_ref)
            .unwrap();
    let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
    terminal.restore_managed_agent("saved-worker".into(), crate::agents::AgentKind::Codex);
    terminal.set_persisted_agent_session(session.clone());
    terminal.pending_agent_resume_plan = Some(plan);

    assert!(!app.start_pending_agent_resumes(true));
    assert!(app.terminal_runtimes.get(&terminal_id).is_none());
    let terminal = &app.state.terminals[&terminal_id];
    assert_eq!(
        terminal.persisted_agent_session,
        Some(session),
        "a shell that never started must not erase the saved conversation"
    );
    assert_eq!(terminal.agent_name.as_deref(), Some("saved-worker"));
}
