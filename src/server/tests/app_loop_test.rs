use super::*;
use crate::server::app_state::Mode;
use crate::server::workspaces::Workspace;

#[test]
fn hidden_render_attempt_keeps_presentation_cadence_available() {
    let (mut app, _) = test_app_with_pane();
    let initial_presentation = Instant::now();
    app.record_render_attempt(initial_presentation, true);

    let hidden_attempt = initial_presentation + MIN_RENDER_INTERVAL;
    app.record_render_attempt(hidden_attempt, false);
    let foreground_echo = hidden_attempt + Duration::from_millis(1);

    assert!(!app.can_render_now(foreground_echo));
    assert!(app.can_present_now(foreground_echo));
}

#[test]
fn interrupted_detached_process_wait_keeps_child_for_retry() {
    let interrupted = std::io::Error::new(std::io::ErrorKind::Interrupted, "test interrupt");

    assert!(retain_detached_process_after_wait(42, Err(interrupted)));
}

fn test_app_with_pane() -> (App, crate::utils::ids::PaneId) {
    let mut app = App::new(
        &crate::utils::config::Config::default(),
        crate::server::app::AppPolicy::TEST,
        None,
        tokio::sync::mpsc::unbounded_channel().1,
        crate::server::api::EventHub::default(),
    );
    let ws = Workspace::test_new("test");
    let pane_id = ws.tabs[0].root_pane;
    app.state.workspaces.push(ws);
    app.state.active = Some(0);
    app.state
        .view
        .pane_infos
        .push(crate::server::workspaces::layout::PaneInfo {
            id: pane_id,
            rect: ratatui::layout::Rect::new(0, 0, 80, 24),
            inner_rect: ratatui::layout::Rect::new(0, 0, 80, 24),
            scrollbar_rect: None,
            borders: ratatui::widgets::Borders::NONE,
            is_focused: true,
        });
    (app, pane_id)
}

#[test]
fn restored_active_workspace_survives_an_earlier_workspace_dropped_during_restore() {
    with_restored_workspaces(true, false, Some(2), 2, |app| {
        let active_id = app
            .state
            .active
            .map(|idx| app.state.workspaces[idx].id.clone());
        let selected_id = app.state.workspaces[app.state.selected].id.clone();
        assert_eq!(app.state.workspaces.len(), 2);
        assert_eq!(active_id.as_deref(), Some("w-last"));
        assert_eq!(selected_id, "w-last");
    });
}

#[test]
fn restored_workspace_focus_preserves_selection_and_fallbacks() {
    // (prune first, legacy IDs, saved active/selected, expected active/selected names)
    for (prune, legacy, active, selected, expected_active, expected_selected) in [
        (true, false, Some(1), 1, Some("middle"), "middle"),
        (true, false, Some(0), 0, None, "middle"),
        (true, false, None, 1, None, "middle"),
        (false, false, Some(1), 2, Some("middle"), "last"),
        (false, false, Some(99), 99, None, "last"),
        (true, true, Some(1), 1, Some("middle"), "middle"),
        (false, true, Some(2), 1, Some("last"), "middle"),
    ] {
        with_restored_workspaces(prune, legacy, active, selected, |app| {
            let active_name = app
                .state
                .active
                .map(|idx| app.state.workspaces[idx].custom_name.as_deref().unwrap());
            assert_eq!(active_name, expected_active);
            assert_eq!(
                app.state.workspaces[app.state.selected]
                    .custom_name
                    .as_deref(),
                Some(expected_selected)
            );
            assert_eq!(
                app.state.mode,
                if expected_active.is_some() {
                    Mode::Terminal
                } else {
                    Mode::Navigate
                }
            );
            assert_eq!(app.last_focus.map(|(idx, _)| idx), app.state.active);
            if let Some((idx, pane_id)) = app.last_focus {
                assert_eq!(app.state.workspaces[idx].focused_pane_id(), Some(pane_id));
            }
        });
    }
}

fn with_restored_workspaces(
    prune_first: bool,
    legacy_ids: bool,
    active: Option<usize>,
    selected: usize,
    check: impl FnOnce(&App),
) {
    let _guard = crate::utils::config::test_config_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root_dir = std::env::temp_dir().join(format!(
        "bus-restore-index-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root_dir).unwrap();
    let root = root_dir.canonicalize().unwrap();
    let previous_root = std::env::var_os("BUS_DATA_DIR");
    let previous_session = std::env::var_os("HERDR_SESSION");
    std::env::set_var("BUS_DATA_DIR", &root);
    std::env::remove_var("HERDR_SESSION");
    let config_dir = crate::utils::config::config_dir();
    std::fs::create_dir_all(&config_dir).unwrap();
    let cwd = root.display().to_string();
    let tab = serde_json::json!({
        "layout": {"Pane": 1},
        "panes": {"1": {"cwd": cwd}},
        "zoomed": false,
    });
    let first_tabs = if prune_first {
        vec![]
    } else {
        vec![tab.clone()]
    };
    let mut snapshot = serde_json::json!({
        "version": 3,
        "workspaces": [
            {"id": "w-dropped", "custom_name": "first", "identity_cwd": cwd, "tabs": first_tabs},
            {"id": "w-middle", "custom_name": "middle", "identity_cwd": cwd, "tabs": [tab.clone()]},
            {"id": "w-last", "custom_name": "last", "identity_cwd": cwd, "tabs": [tab]},
        ],
        "active": active,
        "selected": selected,
    });
    if legacy_ids {
        for workspace in snapshot["workspaces"].as_array_mut().unwrap() {
            workspace.as_object_mut().unwrap().remove("id");
        }
    }
    std::fs::write(config_dir.join("session.json"), snapshot.to_string()).unwrap();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let _enter = runtime.enter();
    let mut app = App::new(
        &crate::utils::config::Config::default(),
        crate::server::app::AppPolicy {
            restore_session: true,
            persist_session: false,
        },
        None,
        tokio::sync::mpsc::unbounded_channel().1,
        crate::server::api::EventHub::default(),
    );
    let terminal_ids: Vec<_> = app.state.terminals.keys().cloned().collect();
    for terminal_id in terminal_ids {
        app.shutdown_terminal_runtime(terminal_id);
    }
    match previous_root {
        Some(value) => std::env::set_var("BUS_DATA_DIR", value),
        None => std::env::remove_var("BUS_DATA_DIR"),
    }
    if let Some(value) = previous_session {
        std::env::set_var("HERDR_SESSION", value);
    }

    let _ = std::fs::remove_dir_all(&root);

    check(&app);
}
