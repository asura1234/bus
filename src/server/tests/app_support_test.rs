use super::*;

use crate::config::Config;

use crate::detect::{Agent, AgentState};

use crate::workspace::Workspace;

use std::sync::Mutex;

fn test_app() -> App {
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(
        &Config::default(),
        crate::app::AppPolicy::TEST,
        None,
        api_rx,
        crate::api::EventHub::default(),
    );
    app.state.default_shell = exiting_test_command().into();
    app
}

fn unique_temp_path(name: &str) -> std::path::PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("herdr-{name}-{}-{stamp}", std::process::id()))
}

fn config_env_lock() -> &'static Mutex<()> {
    crate::config::test_config_env_lock()
}

fn temp_config_path(name: &str) -> std::path::PathBuf {
    let unique = format!(
        "herdr-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    std::env::temp_dir().join(unique).join("config.toml")
}

#[test]
fn notification_show_api_creates_herdr_toast_with_position() {
    let mut app = test_app();
    app.state.toast_config.delivery = crate::config::ToastDelivery::Herdr;

    let response =
        app.handle_api_request_after_internal_events_drained(crate::api::schema::Request {
            id: "notify".into(),
            method: crate::api::schema::Method::NotificationShow(
                crate::api::schema::NotificationShowParams {
                    title: "build failed".into(),
                    body: Some("api workspace".into()),
                    position: Some(crate::config::ToastHerdrPosition::TopLeft),
                    sound: crate::api::schema::NotificationShowSound::None,
                },
            ),
        });

    let parsed: crate::api::schema::SuccessResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(
        parsed.result,
        crate::api::schema::ResponseResult::NotificationShow {
            shown: true,
            reason: crate::api::schema::NotificationShowReason::Shown,
        }
    );
    let toast = app.state.toast.as_ref().expect("api toast");
    assert_eq!(toast.title, "build failed");
    assert_eq!(toast.context, "api workspace");
    assert_eq!(
        toast.position,
        Some(crate::config::ToastHerdrPosition::TopLeft)
    );
    assert!(app.toast_deadline.is_some());
}

#[test]
fn notification_show_api_respects_off_delivery() {
    let mut app = test_app();
    app.state.toast_config.delivery = crate::config::ToastDelivery::Off;

    let response =
        app.handle_api_request_after_internal_events_drained(crate::api::schema::Request {
            id: "notify".into(),
            method: crate::api::schema::Method::NotificationShow(
                crate::api::schema::NotificationShowParams {
                    title: "build failed".into(),
                    body: None,
                    position: None,
                    sound: crate::api::schema::NotificationShowSound::None,
                },
            ),
        });

    let parsed: crate::api::schema::SuccessResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(
        parsed.result,
        crate::api::schema::ResponseResult::NotificationShow {
            shown: false,
            reason: crate::api::schema::NotificationShowReason::Disabled,
        }
    );
    assert!(app.state.toast.is_none());
}

#[test]
fn notification_show_api_does_not_replace_existing_toast() {
    let mut app = test_app();
    app.state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    app.state.toast = Some(crate::app::state::ToastNotification {
        kind: crate::app::state::ToastKind::NeedsAttention,
        title: "pi needs attention".to_string(),
        context: "background · 2".to_string(),
        position: None,
        target: None,
    });

    let response =
        app.handle_api_request_after_internal_events_drained(crate::api::schema::Request {
            id: "notify".into(),
            method: crate::api::schema::Method::NotificationShow(
                crate::api::schema::NotificationShowParams {
                    title: "build failed".into(),
                    body: None,
                    position: None,
                    sound: crate::api::schema::NotificationShowSound::None,
                },
            ),
        });

    let parsed: crate::api::schema::SuccessResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(
        parsed.result,
        crate::api::schema::ResponseResult::NotificationShow {
            shown: false,
            reason: crate::api::schema::NotificationShowReason::Busy,
        }
    );
    assert_eq!(
        app.state.toast.as_ref().map(|toast| toast.title.as_str()),
        Some("pi needs attention")
    );
}

#[test]
fn notification_show_api_is_rate_limited() {
    let mut app = test_app();
    app.state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    app.mark_api_notification_shown(Instant::now());

    let response =
        app.handle_api_request_after_internal_events_drained(crate::api::schema::Request {
            id: "notify".into(),
            method: crate::api::schema::Method::NotificationShow(
                crate::api::schema::NotificationShowParams {
                    title: "build failed".into(),
                    body: None,
                    position: None,
                    sound: crate::api::schema::NotificationShowSound::None,
                },
            ),
        });

    let parsed: crate::api::schema::SuccessResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(
        parsed.result,
        crate::api::schema::ResponseResult::NotificationShow {
            shown: false,
            reason: crate::api::schema::NotificationShowReason::RateLimited,
        }
    );
    assert!(app.state.toast.is_none());
}

#[test]
fn internal_event_drain_limits_work_per_tick() {
    let mut app = test_app();
    for _ in 0..=APP_EVENT_DRAIN_LIMIT {
        app.event_tx
            .try_send(AppEvent::ClipboardWrite {
                content: Vec::new(),
            })
            .unwrap();
    }

    assert!(app.drain_internal_events());

    assert!(app.event_rx.try_recv().is_ok());
}

#[test]
fn api_request_drains_all_pending_internal_events_before_reading_state() {
    let mut app = test_app();
    for _ in 0..=APP_EVENT_DRAIN_LIMIT {
        app.event_tx
            .try_send(AppEvent::ClipboardWrite {
                content: Vec::new(),
            })
            .unwrap();
    }

    let response = app.handle_api_request(crate::api::schema::Request {
        id: "req_server_stop_after_events".into(),
        method: crate::api::schema::Method::ServerStop(crate::api::schema::EmptyParams::default()),
    });
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();

    assert_eq!(response["result"]["type"], "ok");
    assert!(app.event_rx.try_recv().is_err());
}

#[test]
fn read_only_api_requests_do_not_force_rerender() {
    let read_only = crate::api::schema::Request {
        id: "req_1".into(),
        method: crate::api::schema::Method::WorkspaceList(
            crate::api::schema::EmptyParams::default(),
        ),
    };
    let mutating = crate::api::schema::Request {
        id: "req_2".into(),
        method: crate::api::schema::Method::WorkspaceFocus(crate::api::schema::WorkspaceTarget {
            workspace_id: "w1".into(),
        }),
    };
    let pane_rename = crate::api::schema::Request {
        id: "req_3".into(),
        method: crate::api::schema::Method::PaneRename(crate::api::schema::PaneRenameParams {
            pane_id: "w1:p1".into(),
            label: Some("logs".into()),
        }),
    };
    let pane_swap = crate::api::schema::Request {
        id: "req_6".into(),
        method: crate::api::schema::Method::PaneSwap(crate::api::schema::PaneSwapParams {
            pane_id: Some("w1:p1".into()),
            direction: Some(crate::api::schema::PaneDirection::Right),
            ..crate::api::schema::PaneSwapParams::default()
        }),
    };
    let pane_focus_direction = crate::api::schema::Request {
        id: "req_7".into(),
        method: crate::api::schema::Method::PaneFocusDirection(
            crate::api::schema::PaneFocusDirectionParams {
                pane_id: Some("w1:p1".into()),
                direction: crate::api::schema::PaneDirection::Right,
            },
        ),
    };
    let pane_resize = crate::api::schema::Request {
        id: "req_8".into(),
        method: crate::api::schema::Method::PaneResize(crate::api::schema::PaneResizeParams {
            pane_id: Some("w1:p1".into()),
            direction: crate::api::schema::PaneDirection::Right,
            amount: Some(0.05),
        }),
    };

    assert!(!crate::api::request_changes_ui(&read_only));
    assert!(crate::api::request_changes_ui(&mutating));
    assert!(crate::api::request_changes_ui(&pane_rename));
    assert!(crate::api::request_changes_ui(&pane_swap));
    assert!(crate::api::request_changes_ui(&pane_focus_direction));
    assert!(crate::api::request_changes_ui(&pane_resize));
}

#[test]
fn workspace_create_response_includes_initial_tab_and_root_pane() {
    let mut app = test_app();
    app.state.workspaces = vec![Workspace::test_new("api-root-pane")];
    app.state.ensure_test_terminals();
    app.state.active = Some(0);
    app.state.selected = 0;

    let crate::api::schema::ResponseResult::WorkspaceCreated {
        workspace,
        tab,
        root_pane,
    } = app.workspace_created_result(0).unwrap()
    else {
        panic!("expected workspace_created response");
    };

    assert_eq!(workspace.label, "api-root-pane");
    assert_eq!(tab.workspace_id, workspace.workspace_id);
    assert_eq!(root_pane.workspace_id, workspace.workspace_id);
    assert_eq!(root_pane.tab_id, tab.tab_id);
    assert!(root_pane.terminal_id.starts_with("term_"));
    assert_ne!(root_pane.terminal_id, root_pane.pane_id);
}

#[test]
fn tab_create_response_includes_root_pane() {
    let mut app = test_app();
    let mut workspace = Workspace::test_new("api-tab-root-pane");
    workspace.test_add_tab(None);
    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    app.state.active = Some(0);
    app.state.selected = 0;

    let crate::api::schema::ResponseResult::TabCreated { tab, root_pane } =
        app.tab_created_result(0, 1).unwrap()
    else {
        panic!("expected tab_created response");
    };

    assert_eq!(tab.workspace_id, root_pane.workspace_id);
    assert_eq!(root_pane.tab_id, tab.tab_id);
    assert_eq!(tab.pane_count, 1);
}

#[test]
fn tab_info_number_uses_stable_public_tab_number() {
    let mut app = test_app();
    let mut workspace = Workspace::test_new("api-tab-public-number");
    let removed_tab = workspace.test_add_tab(None);
    let survivor_tab = workspace.test_add_tab(None);
    let survivor_pane = workspace.tabs[survivor_tab].root_pane;
    assert!(workspace.close_tab(removed_tab));
    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    app.state.active = Some(0);
    app.state.selected = 0;
    let survivor_idx = app.state.workspaces[0]
        .find_tab_index_for_pane(survivor_pane)
        .unwrap();

    let tab = app.tab_info(0, survivor_idx).unwrap();

    assert_eq!(tab.tab_id, format!("{}:t3", app.state.workspaces[0].id));
    assert_eq!(tab.number, 3);
    assert_eq!(tab.label, "2");
}

#[test]
fn legacy_bare_tab_id_uses_tab_position_not_public_tab_number() {
    let mut app = test_app();
    let mut workspace = Workspace::test_new("legacy-tab-id");
    let removed_tab = workspace.test_add_tab(None);
    workspace.test_add_tab(None);
    let public_four_tab = workspace.test_add_tab(None);
    let fourth_position_tab = workspace.test_add_tab(None);
    let public_four_pane = workspace.tabs[public_four_tab].root_pane;
    let fourth_position_pane = workspace.tabs[fourth_position_tab].root_pane;
    assert!(workspace.close_tab(removed_tab));
    app.state.workspaces = vec![workspace];

    let public_four_idx = app.state.workspaces[0]
        .find_tab_index_for_pane(public_four_pane)
        .unwrap();
    let fourth_position_idx = app.state.workspaces[0]
        .find_tab_index_for_pane(fourth_position_pane)
        .unwrap();

    assert_eq!(app.state.workspaces[0].tabs[public_four_idx].number, 4);
    assert_eq!(app.state.workspaces[0].tabs[fourth_position_idx].number, 5);
    assert_eq!(
        app.parse_tab_id(&format!("{}:t4", app.state.workspaces[0].id)),
        Some((0, public_four_idx))
    );
    assert_eq!(
        app.parse_tab_id(&format!("{}:4", app.state.workspaces[0].id)),
        Some((0, fourth_position_idx))
    );
}

#[test]
fn workspace_creation_in_navigate_mode_uses_selected_workspace_seed_cwd() {
    let mut app = test_app();
    let mut first = Workspace::test_new("herdr");
    first.identity_cwd = std::path::PathBuf::from("/tmp/herdr");
    let mut second = Workspace::test_new("pion");
    second.identity_cwd = std::path::PathBuf::from("/tmp/pion");

    app.state.workspaces = vec![first, second];
    app.state.active = Some(0);
    app.state.selected = 1;
    app.state.mode = Mode::Navigate;

    let ws_idx = app.workspace_creation_source().unwrap();
    let seed_cwd = app.seed_cwd_from_workspace(ws_idx).unwrap();

    assert_eq!(ws_idx, 1);
    assert_eq!(seed_cwd, std::path::PathBuf::from("/tmp/pion"));
}

#[test]
fn new_terminal_cwd_follow_uses_source_cwd() {
    let cwd = creation::resolve_new_terminal_cwd(
        &crate::config::NewTerminalCwdConfig::Follow,
        Some(std::path::PathBuf::from("/tmp/herdr-source")),
    );

    assert_eq!(cwd, std::path::PathBuf::from("/tmp/herdr-source"));
}

#[test]
fn new_terminal_cwd_follow_without_source_uses_home() {
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
        return;
    };

    let cwd =
        creation::resolve_new_terminal_cwd(&crate::config::NewTerminalCwdConfig::Follow, None);

    assert_eq!(cwd, home);
}

#[test]
fn new_terminal_cwd_path_uses_configured_path() {
    let cwd = creation::resolve_new_terminal_cwd(
        &crate::config::NewTerminalCwdConfig::Path("/tmp/herdr-fixed".into()),
        Some(std::path::PathBuf::from("/tmp/herdr-source")),
    );

    assert_eq!(cwd, std::path::PathBuf::from("/tmp/herdr-fixed"));
}

#[test]
fn server_stop_request_sets_should_quit_flag() {
    let mut app = test_app();

    let response = app.handle_api_request(crate::api::schema::Request {
        id: "req_server_stop".into(),
        method: crate::api::schema::Method::ServerStop(crate::api::schema::EmptyParams::default()),
    });
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();

    assert_eq!(response["result"]["type"], "ok");
    assert!(app.state.should_quit);
}

#[test]
fn pane_rename_request_sets_and_clears_manual_label() {
    let mut app = test_app();
    let workspace = Workspace::test_new("api-pane-rename");
    let pane = workspace.tabs[0].root_pane;
    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    app.state.active = Some(0);
    app.state.selected = 0;

    let pane_id = app.pane_info(0, pane).unwrap().pane_id;
    let response = app.handle_api_request(crate::api::schema::Request {
        id: "req_pane_rename".into(),
        method: crate::api::schema::Method::PaneRename(crate::api::schema::PaneRenameParams {
            pane_id: pane_id.clone(),
            label: Some("reviewer".into()),
        }),
    });
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();

    assert_eq!(response["result"]["type"], "pane_info");
    assert_eq!(response["result"]["pane"]["label"], "reviewer");
    let terminal_id = app.state.workspaces[0]
        .pane_state(pane)
        .unwrap()
        .attached_terminal_id
        .clone();
    assert_eq!(
        app.state
            .terminals
            .get(&terminal_id)
            .unwrap()
            .manual_label
            .as_deref(),
        Some("reviewer")
    );

    let response = app.handle_api_request(crate::api::schema::Request {
        id: "req_pane_rename_clear".into(),
        method: crate::api::schema::Method::PaneRename(crate::api::schema::PaneRenameParams {
            pane_id,
            label: None,
        }),
    });
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();

    assert_eq!(response["result"]["type"], "pane_info");
    assert!(response["result"]["pane"].get("label").is_none());
    assert!(app
        .state
        .terminals
        .get(&terminal_id)
        .unwrap()
        .manual_label
        .is_none());
}

#[test]
fn terminal_and_agent_targets_treat_terminal_ids_differently() {
    let mut app = test_app();
    let workspace = Workspace::test_new("terminal-target-id");
    let pane = workspace.tabs[0].root_pane;
    let terminal_id = workspace.terminal_id(pane).unwrap().to_string();
    app.state.workspaces = vec![workspace];
    app.state.active = Some(0);
    app.state.selected = 0;

    let resolved = app.resolve_terminal_target(&terminal_id).unwrap();
    assert_eq!(resolved.pane_id, pane);
    assert_eq!(resolved.terminal_id, terminal_id);

    assert!(matches!(
        app.resolve_agent_target(&resolved.terminal_id),
        Err(crate::app::terminal_targets::TerminalTargetError::NotFound { .. })
    ));
}

#[test]
fn agent_target_rejects_a_pane_that_only_has_a_launch_command() {
    let mut app = test_app();
    let workspace = Workspace::test_new("terminal-target-command");
    let pane = workspace.tabs[0].root_pane;
    let terminal_id = workspace.terminal_id(pane).unwrap().clone();
    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    app.state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .launch_argv = Some(vec!["just".into(), "dev".into()]);
    let pane_id = app.public_pane_id(0, pane).unwrap();

    assert!(app.resolve_terminal_target(&pane_id).is_ok());
    assert!(matches!(
        app.resolve_agent_target(&pane_id),
        Err(crate::app::terminal_targets::TerminalTargetError::NotFound { .. })
    ));
}

#[test]
fn terminal_target_resolves_pane_id_for_an_agent() {
    let mut app = test_app();
    let workspace = Workspace::test_new("terminal-target-pane");
    let pane = workspace.tabs[0].root_pane;
    let terminal_id = workspace.terminal_id(pane).unwrap().to_string();
    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    let attached_terminal_id = app.state.workspaces[0].terminal_id(pane).cloned().unwrap();
    app.state
        .terminals
        .get_mut(&attached_terminal_id)
        .unwrap()
        .set_detected_state(
            Some(crate::detect::Agent::Pi),
            crate::detect::AgentState::Idle,
        );
    app.state.active = Some(0);
    app.state.selected = 0;
    let pane_id = app.public_pane_id(0, pane).unwrap();

    let resolved = app.resolve_terminal_target(&pane_id).unwrap();

    assert_eq!(resolved.pane_id, pane);
    assert_eq!(resolved.terminal_id, terminal_id);
}

#[test]
fn terminal_target_resolves_unique_agent_name() {
    let mut app = test_app();
    let workspace = Workspace::test_new("terminal-target-name");
    let pane = workspace.tabs[0].root_pane;
    let terminal_id = workspace.terminal_id(pane).unwrap().to_string();
    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    let attached_terminal_id = app.state.workspaces[0]
        .pane_state(pane)
        .unwrap()
        .attached_terminal_id
        .clone();
    app.state
        .terminals
        .get_mut(&attached_terminal_id)
        .unwrap()
        .set_agent_name("reviewer".into());
    app.state.active = Some(0);
    app.state.selected = 0;

    let resolved = app.resolve_terminal_target("reviewer").unwrap();

    assert_eq!(resolved.pane_id, pane);
    assert_eq!(resolved.terminal_id, terminal_id);
}

#[test]
fn agent_target_treats_legacy_pane_syntax_as_a_name() {
    let mut app = test_app();
    let workspace = Workspace::test_new("agent-target-name");
    let pane = workspace.tabs[0].root_pane;
    let terminal_id = workspace.terminal_id(pane).unwrap().clone();
    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
    terminal.set_detected_state(
        Some(crate::detect::Agent::Pi),
        crate::detect::AgentState::Idle,
    );
    terminal.set_agent_name("p_1".into());

    let resolved = app.resolve_agent_target("p_1").unwrap();

    assert_eq!(resolved.pane_id, pane);
    assert_eq!(resolved.terminal_id, terminal_id.to_string());
}

#[test]
fn terminal_target_reports_missing_target() {
    let mut app = test_app();
    app.state.workspaces = vec![Workspace::test_new("terminal-target-missing")];
    app.state.active = Some(0);
    app.state.selected = 0;

    let err = app.resolve_terminal_target("missing-agent").unwrap_err();

    assert_eq!(
        err,
        crate::app::terminal_targets::TerminalTargetError::NotFound {
            target: "missing-agent".into()
        }
    );
}

#[test]
fn terminal_target_reports_ambiguous_duplicate_agent_name() {
    let mut app = test_app();
    let mut workspace = Workspace::test_new("terminal-target-ambiguous");
    let first = workspace.tabs[0].root_pane;
    let second = workspace.test_split(ratatui::layout::Direction::Horizontal);
    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    let first_terminal_id = app.state.workspaces[0]
        .pane_state(first)
        .unwrap()
        .attached_terminal_id
        .clone();
    app.state
        .terminals
        .get_mut(&first_terminal_id)
        .unwrap()
        .set_agent_name("worker".into());
    let second_terminal_id = app.state.workspaces[0]
        .pane_state(second)
        .unwrap()
        .attached_terminal_id
        .clone();
    app.state
        .terminals
        .get_mut(&second_terminal_id)
        .unwrap()
        .set_agent_name("worker".into());
    app.state.active = Some(0);
    app.state.selected = 0;

    let err = app.resolve_terminal_target("worker").unwrap_err();

    let crate::app::terminal_targets::TerminalTargetError::Ambiguous { target, candidates } = err
    else {
        panic!("expected ambiguous terminal target");
    };
    assert_eq!(target, "worker");
    assert_eq!(candidates.len(), 2);
    assert!(candidates.iter().all(|candidate| {
        candidate.terminal_id.starts_with("term_")
            && candidate.pane_id.starts_with(&app.state.workspaces[0].id)
            && candidate.workspace_id == app.state.workspaces[0].id
            && candidate.cwd.is_some()
    }));
}

#[tokio::test]
async fn pane_split_request_focuses_new_pane_when_requested() {
    let _guard = config_env_lock().lock().unwrap();
    let original_shell = std::env::var_os("SHELL");
    std::env::set_var("SHELL", exiting_test_command());

    let mut app = test_app();
    let mut workspace = Workspace::test_new("api-pane-split-focus-background-tab");
    let background_tab = workspace.test_add_tab(Some("worker"));
    workspace.switch_tab(0);
    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    app.state.active = Some(0);
    app.state.selected = 0;

    let target_pane = app.state.workspaces[0].tabs[background_tab].root_pane;
    let target_pane_id = app.pane_info(0, target_pane).unwrap().pane_id;
    let target_tab_id = app.public_tab_id(0, background_tab).unwrap();

    let response = app.handle_api_request(crate::api::schema::Request {
        id: "req_pane_split_focus_background_tab".into(),
        method: crate::api::schema::Method::PaneSplit(crate::api::schema::PaneSplitParams {
            workspace_id: None,
            target_pane_id: Some(target_pane_id),
            direction: crate::api::schema::SplitDirection::Right,
            ratio: None,
            cwd: None,
            focus: true,
            right_click: Default::default(),
            env: Default::default(),
        }),
    });
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();

    assert_eq!(response["result"]["type"], "pane_info");
    assert_eq!(response["result"]["pane"]["tab_id"], target_tab_id);
    assert_eq!(response["result"]["pane"]["focused"], true);
    assert_eq!(app.state.active, Some(0));
    assert_eq!(app.state.workspaces[0].active_tab, background_tab);

    let runtimes: Vec<_> = app.terminal_runtimes.drain().collect();
    for (_terminal_id, runtime) in runtimes {
        runtime.shutdown();
    }
    match original_shell {
        Some(value) => std::env::set_var("SHELL", value),
        None => std::env::remove_var("SHELL"),
    }
}

#[tokio::test]
async fn pane_split_request_applies_ratio() {
    let _guard = config_env_lock().lock().unwrap();
    let original_shell = std::env::var_os("SHELL");
    std::env::set_var("SHELL", exiting_test_command());

    let mut app = test_app();
    let workspace = Workspace::test_new("api-pane-split-ratio");
    let target_pane = workspace.tabs[0].root_pane;
    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    app.state.active = Some(0);
    app.state.selected = 0;

    let target_pane_id = app.pane_info(0, target_pane).unwrap().pane_id;

    let response = app.handle_api_request(crate::api::schema::Request {
        id: "req_pane_split_ratio".into(),
        method: crate::api::schema::Method::PaneSplit(crate::api::schema::PaneSplitParams {
            workspace_id: None,
            target_pane_id: Some(target_pane_id),
            direction: crate::api::schema::SplitDirection::Right,
            ratio: Some(0.333),
            cwd: None,
            focus: false,
            right_click: crate::api::schema::PaneRightClickTarget::Pane,
            env: Default::default(),
        }),
    });
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();

    assert_eq!(response["result"]["type"], "pane_info");
    let splits = app.state.workspaces[0].tabs[0]
        .layout
        .splits(ratatui::layout::Rect::new(0, 0, 100, 20));
    assert_eq!(splits.len(), 1);
    assert!((splits[0].ratio - 0.333).abs() < f32::EPSILON);
    let response_pane_id = response["result"]["pane"]["pane_id"].as_str().unwrap();
    let (_, response_pane_id) = app.parse_pane_id(response_pane_id).unwrap();
    assert!(
        app.state.workspaces[0]
            .pane_state(response_pane_id)
            .unwrap()
            .right_click_passthrough
    );

    let runtimes: Vec<_> = app.terminal_runtimes.drain().collect();
    for (_terminal_id, runtime) in runtimes {
        runtime.shutdown();
    }
    match original_shell {
        Some(value) => std::env::set_var("SHELL", value),
        None => std::env::remove_var("SHELL"),
    }
}

#[tokio::test]
async fn pane_split_request_uses_active_focused_pane_when_target_is_omitted() {
    let _guard = config_env_lock().lock().unwrap();
    let original_shell = std::env::var_os("SHELL");
    std::env::set_var("SHELL", exiting_test_command());

    let mut app = test_app();
    let workspace = Workspace::test_new("api-pane-split-current");
    let target_pane = workspace.tabs[0].root_pane;
    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    app.state.active = Some(0);
    app.state.selected = 0;
    app.state.focus_pane_in_workspace(0, target_pane);

    let response = app.handle_api_request(crate::api::schema::Request {
        id: "req_pane_split_current".into(),
        method: crate::api::schema::Method::PaneSplit(crate::api::schema::PaneSplitParams {
            workspace_id: None,
            target_pane_id: None,
            direction: crate::api::schema::SplitDirection::Right,
            ratio: None,
            cwd: None,
            focus: false,
            right_click: Default::default(),
            env: Default::default(),
        }),
    });
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();

    assert_eq!(response["result"]["type"], "pane_info");
    assert_eq!(app.state.workspaces[0].tabs[0].layout.pane_count(), 2);
    assert_eq!(
        app.state.workspaces[0].tabs[0].layout.focused(),
        target_pane
    );

    let runtimes: Vec<_> = app.terminal_runtimes.drain().collect();
    for (_terminal_id, runtime) in runtimes {
        runtime.shutdown();
    }
    match original_shell {
        Some(value) => std::env::set_var("SHELL", value),
        None => std::env::remove_var("SHELL"),
    }
}

#[tokio::test]
async fn unavailable_agent_start_does_not_mutate_topology() {
    let mut app = test_app();
    let workspace = Workspace::test_new("agent-start-target");
    let root = workspace.tabs[0].root_pane;
    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    app.state.active = Some(0);
    app.state.selected = 0;
    let pane_id = app.pane_info(0, root).unwrap().pane_id;

    let response = app.handle_api_request(crate::api::schema::Request {
        id: "req_agent_start_target".into(),
        method: crate::api::schema::Method::AgentStart(crate::api::schema::AgentStartParams {
            name: "worker".into(),
            kind: "pi".into(),
            pane_id,
            args: Vec::new(),
            timeout_ms: Some(1_000),
        }),
    });
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();

    assert_eq!(response["error"]["code"], "agent_pane_unavailable");
    assert_eq!(app.state.workspaces[0].tabs[0].layout.pane_count(), 1);
    assert_eq!(app.state.workspaces[0].focused_pane_id(), Some(root));
}

#[tokio::test]
async fn failed_agent_start_input_rolls_back_and_can_retry() {
    let mut app = test_app();
    let workspace = Workspace::test_new("agent-start-input-failure");
    let root = workspace.tabs[0].root_pane;
    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    app.state.active = Some(0);
    app.state.selected = 0;
    let pane_id = app.pane_info(0, root).unwrap().pane_id;
    let terminal_id = app.state.workspaces[0].tabs[0].panes[&root]
        .attached_terminal_id
        .clone();
    app.state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .set_manual_label("shell".into());
    let (runtime, mut receiver) =
        crate::terminal::TerminalRuntime::test_with_channel_capacity(80, 24, 1);
    runtime
        .try_send_bytes(bytes::Bytes::from_static(b"occupied"))
        .unwrap();
    app.terminal_runtimes.insert(terminal_id.clone(), runtime);

    let request = || crate::api::schema::Request {
        id: "req_agent_start_input".into(),
        method: crate::api::schema::Method::AgentStart(crate::api::schema::AgentStartParams {
            name: "worker".into(),
            kind: "codex".into(),
            pane_id: pane_id.clone(),
            args: vec!["resume".into(), "codex-session".into()],
            timeout_ms: Some(4_000),
        }),
    };
    let response = app.handle_api_request(request());
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["error"]["code"], "agent_start_input_failed");
    assert_eq!(app.state.terminals[&terminal_id].agent_name, None);
    assert!(app.state.terminals[&terminal_id]
        .persisted_agent_session
        .is_none());
    assert_eq!(
        app.state.terminals[&terminal_id].manual_label.as_deref(),
        Some("shell")
    );

    assert_eq!(
        receiver.try_recv().unwrap(),
        bytes::Bytes::from_static(b"occupied")
    );
    let retry = app.handle_api_request(request());
    let retry: serde_json::Value = serde_json::from_str(&retry).unwrap();
    assert_eq!(retry["result"]["type"], "agent_started");
    assert_eq!(
        retry["result"]["agent"]["agent_session"],
        serde_json::json!({
            "source": "herdr:codex",
            "agent": "codex",
            "kind": "id",
            "value": "codex-session",
        })
    );
    assert_eq!(
        app.state.terminals[&terminal_id].agent_name.as_deref(),
        Some("worker")
    );
    let rename = app.handle_api_request(crate::api::schema::Request {
        id: "req_agent_rename_pending".into(),
        method: crate::api::schema::Method::AgentRename(crate::api::schema::AgentRenameParams {
            target: pane_id,
            name: Some("replacement".into()),
        }),
    });
    let rename: serde_json::Value = serde_json::from_str(&rename).unwrap();
    assert_eq!(rename["error"]["code"], "agent_launch_pending");
    assert_eq!(
        app.state.terminals[&terminal_id].agent_name.as_deref(),
        Some("worker")
    );
}

#[test]
fn pane_close_request_closes_only_the_target_tab_when_other_tabs_exist() {
    let mut app = test_app();
    let mut workspace = Workspace::test_new("api-pane-close");
    let second_tab = workspace.test_add_tab(Some("logs"));
    workspace.switch_tab(second_tab);
    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    app.state.active = Some(0);
    app.state.selected = 0;

    let target_pane = app.state.workspaces[0].tabs[second_tab].root_pane;
    let target_pane_id = app.pane_info(0, target_pane).unwrap().pane_id;

    let response = app.handle_api_request(crate::api::schema::Request {
        id: "req_pane_close".into(),
        method: crate::api::schema::Method::PaneClose(crate::api::schema::PaneTarget {
            pane_id: target_pane_id,
        }),
    });
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();

    assert_eq!(response["result"]["type"], "ok");
    assert_eq!(app.state.workspaces.len(), 1);
    assert_eq!(app.state.workspaces[0].tabs.len(), 1);
    assert_eq!(app.state.workspaces[0].display_name(), "api-pane-close");
}

#[test]
fn pane_close_request_closes_workspace_when_it_removes_the_last_pane() {
    let mut app = test_app();
    let workspace = Workspace::test_new("api-pane-close-last");
    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    app.state.active = Some(0);
    app.state.selected = 0;

    let target_pane = app.state.workspaces[0].tabs[0].root_pane;
    let target_pane_id = app.pane_info(0, target_pane).unwrap().pane_id;

    let response = app.handle_api_request(crate::api::schema::Request {
        id: "req_pane_close_last".into(),
        method: crate::api::schema::Method::PaneClose(crate::api::schema::PaneTarget {
            pane_id: target_pane_id,
        }),
    });
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();

    assert_eq!(response["result"]["type"], "ok");
    assert!(app.state.workspaces.is_empty());
}

#[test]
fn session_dirty_flag_schedules_debounced_save() {
    let mut app = test_app();
    app.policy.persist_session = true;
    app.state.session_dirty = true;

    app.sync_session_save_schedule();

    assert!(!app.state.session_dirty);
    assert!(app.session_save_deadline.is_some());
}

#[test]
fn headless_next_loop_deadline_ignores_resize_poll() {
    let mut app = test_app();
    let now = Instant::now();
    app.session_save_deadline = Some(now + Duration::from_secs(2));

    assert_eq!(
        app.next_headless_loop_deadline(now, false),
        app.session_save_deadline
    );
}

#[test]
fn headless_next_loop_deadline_returns_none_when_resize_poll_is_only_deadline() {
    let mut app = test_app();
    let now = Instant::now();
    app.config_diagnostic_deadline = None;
    app.toast_deadline = None;
    app.session_save_deadline = None;
    app.state.workspaces.clear();

    assert_eq!(app.next_headless_loop_deadline(now, false), None);
}

#[test]
fn due_session_save_starts_background_writer() {
    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    let _bus = crate::config::test_without_bus_env(&_guard);
    let config_home = unique_temp_path("background-session-save");
    std::env::set_var("XDG_CONFIG_HOME", &config_home);
    std::env::remove_var(crate::session::SESSION_ENV_VAR);

    let mut app = test_app();
    app.policy.persist_session = true;
    app.state.workspaces = vec![Workspace::test_new("autosave")];
    app.state.ensure_test_terminals();
    app.session_save_deadline = Some(Instant::now() - Duration::from_secs(1));

    app.start_background_session_save();

    assert!(app.session_save_thread.is_some());
    assert!(app.session_save_deadline.is_none());
    app.save_session_now();
    assert!(crate::session::data_dir().join("session.json").exists());

    std::env::remove_var("XDG_CONFIG_HOME");
    let _ = std::fs::remove_dir_all(config_home);
}

#[test]
fn background_session_save_reschedules_when_writer_is_busy() {
    let mut app = test_app();
    app.policy.persist_session = true;
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    app.session_save_thread = Some(std::thread::spawn(move || {
        let _ = release_rx.recv();
    }));

    app.start_background_session_save();

    assert!(app.session_save_thread.is_some());
    assert!(app.session_save_deadline.is_some());

    release_tx.send(()).unwrap();
    app.policy.persist_session = false;
    app.save_session_now();
}

#[test]
fn final_session_save_joins_background_writer_before_returning() {
    let mut app = test_app();
    app.policy.persist_session = false;
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    app.session_save_thread = Some(std::thread::spawn(move || {
        let _ = release_rx.recv();
        done_tx.send(()).unwrap();
    }));
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        release_tx.send(()).unwrap();
    });

    app.save_session_now();

    releaser.join().unwrap();
    done_rx.try_recv().unwrap();
    assert!(app.session_save_thread.is_none());
}

#[tokio::test]
async fn pane_exit_checkpoint_survives_automatic_workspace_creation_on_shutdown() {
    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    let _bus = crate::config::test_without_bus_env(&_guard);
    let config_home = unique_temp_path("signaled-pane-session-checkpoint");
    std::env::set_var("XDG_CONFIG_HOME", &config_home);
    std::env::remove_var(crate::session::SESSION_ENV_VAR);

    let mut app = test_app();
    app.policy.persist_session = true;
    let mut workspace = Workspace::test_new("preserved");
    let first_pane = workspace.tabs[0].root_pane;
    let second_pane = workspace.test_split(ratatui::layout::Direction::Horizontal);
    app.state.workspaces = vec![workspace];
    app.state.active = Some(0);
    app.state.ensure_test_terminals();

    app.handle_internal_event(AppEvent::PaneDied {
        pane_id: first_pane,
        exit_reason: crate::platform::ChildExitReason::Interrupted,
    });
    app.handle_internal_event(AppEvent::PaneDied {
        pane_id: second_pane,
        exit_reason: crate::platform::ChildExitReason::Interrupted,
    });
    assert!(app.state.workspaces.is_empty());
    assert!(app.ensure_default_workspace());

    app.save_session_on_shutdown();

    let snapshot = crate::persist::load().expect("checkpointed session should survive");
    assert_eq!(snapshot.workspaces.len(), 1);
    assert_eq!(snapshot.workspaces[0].tabs[0].panes.len(), 2);

    std::env::remove_var("XDG_CONFIG_HOME");
    let _ = std::fs::remove_dir_all(config_home);
}

#[test]
fn normal_autosave_replaces_a_signaled_exit_checkpoint() {
    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    let _bus = crate::config::test_without_bus_env(&_guard);
    let config_home = unique_temp_path("signaled-pane-autosave");
    std::env::set_var("XDG_CONFIG_HOME", &config_home);
    std::env::remove_var(crate::session::SESSION_ENV_VAR);

    let mut app = test_app();
    app.policy.persist_session = true;
    let workspace = Workspace::test_new("closed");
    let pane_id = workspace.tabs[0].root_pane;
    app.state.workspaces = vec![workspace];
    app.state.active = Some(0);
    app.state.ensure_test_terminals();

    app.handle_internal_event(AppEvent::PaneDied {
        pane_id,
        exit_reason: crate::platform::ChildExitReason::Interrupted,
    });
    assert!(crate::persist::load().is_some());

    app.start_background_session_save();
    if let Some(thread) = app.session_save_thread.take() {
        thread.join().unwrap();
    }
    app.save_session_on_shutdown();

    assert!(crate::persist::load().is_none());

    std::env::remove_var("XDG_CONFIG_HOME");
    let _ = std::fs::remove_dir_all(config_home);
}

#[test]
fn durable_mutation_after_pane_exit_checkpoint_wins_on_shutdown() {
    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    let _bus = crate::config::test_without_bus_env(&_guard);
    let config_home = unique_temp_path("pane-exit-newer-session-state");
    std::env::set_var("XDG_CONFIG_HOME", &config_home);
    std::env::remove_var(crate::session::SESSION_ENV_VAR);

    for another_interrupted_exit in [false, true] {
        let mut app = test_app();
        app.policy.persist_session = true;
        let workspace = Workspace::test_new("old");
        let pane_id = workspace.tabs[0].root_pane;
        app.state.workspaces = vec![workspace];
        app.state.active = Some(0);
        app.state.ensure_test_terminals();

        app.handle_internal_event(AppEvent::PaneDied {
            pane_id,
            exit_reason: crate::platform::ChildExitReason::Interrupted,
        });
        app.state.workspaces = vec![Workspace::test_new("newer")];
        app.state.active = Some(0);
        app.state.ensure_test_terminals();
        app.state.mark_session_dirty();
        if another_interrupted_exit {
            app.handle_internal_event(AppEvent::PaneDied {
                pane_id: app.state.workspaces[0].tabs[0].root_pane,
                exit_reason: crate::platform::ChildExitReason::Interrupted,
            });
        }
        app.save_session_on_shutdown();

        let snapshot = crate::persist::load().expect("newer session should be saved");
        assert_eq!(snapshot.workspaces.len(), 1);
        assert_eq!(snapshot.workspaces[0].custom_name.as_deref(), Some("newer"));
    }

    std::env::remove_var("XDG_CONFIG_HOME");
    let _ = std::fs::remove_dir_all(config_home);
}

#[tokio::test]
async fn full_internal_event_queue_eventually_applies_working_to_idle_transition() {
    let mut app = test_app();
    let ws = Workspace::test_new("test");
    let pane_id = ws.tabs[0].root_pane;

    app.state.workspaces = vec![ws];
    app.state.ensure_test_terminals();
    app.state.active = Some(0);
    app.state.selected = 0;
    app.state.mode = Mode::Terminal;

    let terminal_id = app.state.workspaces[0]
        .pane_state(pane_id)
        .unwrap()
        .attached_terminal_id
        .clone();
    app.handle_internal_event(AppEvent::StateChanged {
        pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Working,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });
    assert_eq!(
        app.state.terminals.get(&terminal_id).unwrap().state,
        AgentState::Working
    );

    for _ in 0..APP_EVENT_CHANNEL_CAPACITY {
        app.event_tx
            .try_send(AppEvent::ClipboardWrite {
                content: Vec::new(),
            })
            .unwrap();
    }

    let tx = app.event_tx.clone();
    let send = tx.send(AppEvent::StateChanged {
        pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Idle,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });
    tokio::pin!(send);

    let blocked =
        tokio::time::timeout(Duration::from_millis(20), async { (&mut send).await }).await;
    assert!(
        blocked.is_err(),
        "state change sender should wait for queue space instead of failing"
    );

    app.drain_internal_events();

    tokio::time::timeout(Duration::from_millis(50), async { (&mut send).await })
        .await
        .expect("state change should enqueue once queue space is available")
        .expect("app event receiver should still be alive");

    let max_drains = (APP_EVENT_CHANNEL_CAPACITY / APP_EVENT_DRAIN_LIMIT) + 2;
    for _ in 0..max_drains {
        if app.state.terminals.get(&terminal_id).unwrap().state == AgentState::Idle {
            break;
        }
        app.drain_internal_events();
    }

    assert_eq!(
        app.state.terminals.get(&terminal_id).unwrap().state,
        AgentState::Idle,
        "Working→Idle should still apply after temporary queue pressure"
    );
}

#[path = "config_reload_test.rs"]
mod config_reload;
#[path = "startup_test.rs"]
mod startup;
#[path = "theme_test.rs"]
mod theme;
