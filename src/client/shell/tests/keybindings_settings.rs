use super::*;

#[test]
fn manual_client_chrome_preferences_round_trip_per_endpoint() {
    let path = std::env::temp_dir().join(format!(
        "herdr-client-shell-prefs-{}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let config =
        ClientShellConfig::from_config(&Config::default()).with_preferences_path(path.clone());
    let mut state = ClientShellState::new(config);
    state.sidebar_width = 31;
    state.sidebar_width_manual = true;
    state.sidebar_section_split = 0.7;
    state.sidebar_section_split_manual = true;
    state.sidebar_collapsed = true;
    state.sidebar_collapsed_manual = true;
    state.collapsed_groups.insert("repo-two".into());
    state.collapsed_groups.insert("repo-one".into());
    state.persist_chrome_preferences(&mut ClientShellInput::default());

    let reloaded_config =
        ClientShellConfig::from_config(&Config::default()).with_preferences_path(path.clone());
    let reloaded = ClientShellState::new(reloaded_config);
    assert_eq!(reloaded.sidebar_width, 31);
    assert!(reloaded.sidebar_width_manual);
    assert_eq!(reloaded.sidebar_section_split, 0.7);
    assert!(reloaded.sidebar_section_split_manual);
    assert!(reloaded.sidebar_collapsed);
    assert!(reloaded.sidebar_collapsed_manual);
    assert_eq!(
        reloaded.collapsed_groups,
        HashSet::from(["repo-one".to_string(), "repo-two".to_string()])
    );
    std::fs::remove_file(path).expect("remove client chrome preferences");
}

#[test]
fn configured_prefix_is_client_owned_and_renders_its_bar() {
    let config = toml::from_str::<Config>(
        r#"
[keys]
prefix = "ctrl+a"
detach = "prefix+x"
"#,
    )
    .expect("configured keybinds");
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());

    let old_default = state.handle_input_bytes(&[0x02]);
    assert_eq!(
        old_default.requests.len(),
        1,
        "ctrl-b should reach the pane"
    );

    let prefix = state.handle_input_bytes(&[0x01]);
    assert!(prefix.requests.is_empty());
    assert!(prefix.repaint);
    let frame = state.compose(106, 20).expect("prefix frame");
    let text = frame
        .cells
        .chunks(frame.width as usize)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("PREFIX"), "frame: {text:?}");
    assert!(text.contains("ctrl+a"), "frame: {text:?}");

    let detach = state.handle_input_bytes(b"x");
    assert!(detach.detach);
    assert!(detach.requests.is_empty());
}

#[test]
fn prefix_endpoint_action_uses_public_api_with_stable_ids() {
    let mut config = Config::default();
    config.ui.prompt_new_tab_name = false;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(snapshot()));

    assert!(state.handle_input_bytes(&[0x02]).actions.is_empty());
    let create = state.handle_input_bytes(b"c");
    let [ClientShellAction::Endpoint {
        boot_id, request, ..
    }] = &create.actions[..]
    else {
        panic!("expected one endpoint action: {:?}", create.actions);
    };
    assert_eq!(boot_id, "boot-1");
    match &request.method {
        crate::api::schema::Method::TabCreate(params) => {
            assert_eq!(params.workspace_id.as_deref(), Some("ws_1"));
            assert!(params.focus);
        }
        other => panic!("expected tab.create, got {other:?}"),
    }
    assert!(state.pending_requests.contains_key(&request.id));
}

#[test]
fn custom_binding_invokes_only_the_endpoint_manifest_id() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let binding = crate::config::CustomCommandKeybind {
        bindings: crate::config::ActionKeybinds::prefix("z"),
        label: "prefix+z".into(),
        command: "secret-command --token hidden".into(),
        action: crate::config::CustomCommandAction::Shell,
        description: None,
        width: None,
        height: None,
    };
    let mut projection = snapshot();
    projection
        .commands
        .push(crate::protocol::ClientShellCommand {
            command_id: "cmd_0123456789abcdef0123456789abcdef".into(),
            binding_label: binding.label.clone(),
            binding_labels: binding.bindings.labels(),
            action: crate::protocol::ClientShellCommandAction::Shell,
            description: None,
        });
    state.set_snapshot(Box::new(projection));

    let mut outcome = ClientShellInput::default();
    state.record_binding(crate::input::KeybindMatch::Command(binding), &mut outcome);

    let [ClientShellAction::Endpoint { request, .. }] = &outcome.actions[..] else {
        panic!("expected endpoint command invocation");
    };
    let crate::api::schema::Method::CommandInvoke(params) = &request.method else {
        panic!("expected command.invoke");
    };
    assert_eq!(params.command_id, "cmd_0123456789abcdef0123456789abcdef");
    assert_eq!(params.workspace_id.as_deref(), Some("ws_1"));
    assert_eq!(params.tab_id.as_deref(), Some("tab_1"));
    assert_eq!(params.pane_id.as_deref(), Some("pane_1"));
    assert!(!serde_json::to_string(request)
        .unwrap()
        .contains("secret-command"));
}

#[test]
fn unavailable_endpoint_method_is_disabled_without_disconnect() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.set_endpoint_methods(Some(vec!["pane.focus".into()]));
    let mut outcome = ClientShellInput::default();

    state.push_endpoint_method(
        crate::api::schema::Method::WorkspaceFocus(crate::api::schema::WorkspaceTarget {
            workspace_id: "missing".into(),
        }),
        &mut outcome,
    );

    assert!(outcome.actions.is_empty());
    assert!(outcome.repaint);
    let notice = state
        .visible_endpoint_notice
        .as_ref()
        .expect("unsupported action notice");
    assert_eq!(notice.key.kind, ClientEndpointNoticeKind::Unsupported);
    assert_eq!(notice.key.code, "workspace.focus");
    assert!(notice.body.contains("This server"));
    assert!(state.endpoint_error.is_none());

    let mut repeated = ClientShellInput::default();
    state.push_endpoint_method(
        crate::api::schema::Method::WorkspaceFocus(crate::api::schema::WorkspaceTarget {
            workspace_id: "missing".into(),
        }),
        &mut repeated,
    );
    assert!(repeated.actions.is_empty());
    assert!(!repeated.repaint);

    state.compose(106, 20).expect("endpoint notice frame");
    assert!(!state.hits.notification_toast.is_empty());

    state.handle_input_bytes(b"x");
    assert!(state.visible_endpoint_notice.is_some());

    let hit = state.hits.notification_toast;
    let dismissed =
        state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: hit.x,
            row: hit.y,
            modifiers: KeyModifiers::empty(),
        })]);
    assert!(dismissed.repaint);
    assert!(state.visible_endpoint_notice.is_none());
}

#[test]
fn generic_endpoint_failures_and_control_errors_are_visible() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    let mut outcome = ClientShellInput::default();
    state.push_endpoint_method(
        crate::api::schema::Method::WorkspaceFocus(crate::api::schema::WorkspaceTarget {
            workspace_id: "missing".into(),
        }),
        &mut outcome,
    );
    let request_id = match &outcome.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => request.id.clone(),
        other => panic!("expected generic endpoint request, got {other:?}"),
    };
    let rejected_at = std::time::Instant::now();
    let (repaint, actions) = state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Err(ClientShellEndpointError {
            code: Some("not_found".into()),
            message: "workspace no longer exists".into(),
        }),
    );
    assert!(repaint);
    assert!(actions.is_empty());
    let notice = state
        .visible_endpoint_notice
        .as_ref()
        .expect("rejected action notice");
    assert_eq!(notice.key.kind, ClientEndpointNoticeKind::Rejected);
    assert_eq!(notice.key.code, "workspace.focus:not_found");
    assert_eq!(notice.body, "workspace no longer exists");
    let observed_at = std::time::Instant::now();
    assert!(notice.deadline >= rejected_at + std::time::Duration::from_secs(3));
    assert!(notice.deadline <= observed_at + std::time::Duration::from_secs(3));
    assert!(state.endpoint_error.is_none());

    assert!(state.receive_endpoint_error("Paste rejected: too large".into()));
    let notice = state
        .visible_endpoint_notice
        .as_ref()
        .expect("paste rejection notice");
    assert_eq!(notice.key.kind, ClientEndpointNoticeKind::Rejected);
    assert_eq!(notice.key.code, "paste_rejected");
    assert_eq!(notice.body, "Paste rejected: too large");
    assert!(state.receive_endpoint_error("Paste rejected again".into()));
    assert_eq!(
        state
            .visible_endpoint_notice
            .as_ref()
            .map(|notice| notice.body.as_str()),
        Some("Paste rejected again")
    );
}

#[test]
fn endpoint_timeout_is_a_deduplicated_server_notice() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    let mut outcome = ClientShellInput::default();
    state.push_endpoint_method(
        crate::api::schema::Method::WorkspaceFocus(crate::api::schema::WorkspaceTarget {
            workspace_id: "work".into(),
        }),
        &mut outcome,
    );
    let request_id = match &outcome.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => request.id.clone(),
        other => panic!("expected endpoint request, got {other:?}"),
    };

    let (repaint, actions) = state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Err(ClientShellEndpointError {
            code: Some("endpoint_timeout".into()),
            message: "transport timeout".into(),
        }),
    );

    assert!(repaint);
    assert!(actions.is_empty());
    let notice = state
        .visible_endpoint_notice
        .as_ref()
        .expect("timeout notice");
    assert_eq!(notice.key.kind, ClientEndpointNoticeKind::Timeout);
    assert_eq!(notice.key.code, "workspace.focus");
    assert!(notice.body.contains("workspace.focus"));
    assert!(state.endpoint_error.is_none());
}

#[test]
fn endpoint_notice_dedupe_resets_for_a_new_server_boot() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_endpoint_methods(Some(Vec::new()));
    let method = || {
        crate::api::schema::Method::WorkspaceFocus(crate::api::schema::WorkspaceTarget {
            workspace_id: "work".into(),
        })
    };
    let mut first = ClientShellInput::default();
    state.push_endpoint_method(method(), &mut first);
    assert!(first.repaint);

    let mut next_snapshot = snapshot();
    next_snapshot.boot_id = "boot-2".into();
    state.set_snapshot(Box::new(next_snapshot));
    let mut second = ClientShellInput::default();
    state.push_endpoint_method(method(), &mut second);

    assert!(second.repaint);
    assert_eq!(
        state
            .visible_endpoint_notice
            .as_ref()
            .map(|notice| notice.key.boot_id.as_str()),
        Some("boot-2")
    );
}

#[test]
fn custom_binding_missing_from_endpoint_manifest_is_not_forwarded() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    let binding = crate::config::CustomCommandKeybind {
        bindings: crate::config::ActionKeybinds::prefix("z"),
        label: "prefix+z".into(),
        command: "secret-command".into(),
        action: crate::config::CustomCommandAction::Shell,
        description: None,
        width: None,
        height: None,
    };

    let mut outcome = ClientShellInput::default();
    state.record_binding(crate::input::KeybindMatch::Command(binding), &mut outcome);

    assert!(outcome.actions.is_empty());
    assert!(outcome.repaint);
    assert!(state
        .endpoint_error
        .as_deref()
        .is_some_and(|error| error.contains("not available")));
}

#[test]
fn help_overlay_restores_released_search_scroll_and_custom_binding_behavior() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let mut projection = snapshot();
    projection
        .commands
        .push(crate::protocol::ClientShellCommand {
            command_id: "shell-action".into(),
            binding_label: "prefix+z".into(),
            binding_labels: vec!["prefix+z".into()],
            action: crate::protocol::ClientShellCommandAction::Shell,
            description: Some("run shell action".into()),
        });
    state.set_snapshot(Box::new(projection));
    state.set_pane_surface(surface());
    let mut open = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::Help),
        &mut open,
    );
    let initial = state.compose(106, 30).expect("help overlay");
    let text = initial
        .cells
        .chunks(initial.width as usize)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("global"));
    assert!(state.hits.help_max_scroll > 0);
    assert_ne!(state.hits.help_scrollbar, Rect::default());

    state.handle_input_bytes(b"/");
    state.handle_input_bytes(b"shell");
    let custom = state.compose(106, 30).expect("custom help search");
    let text = custom
        .cells
        .chunks(custom.width as usize)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("custom"));
    assert!(text.contains("run shell action"));
    state.handle_input_bytes(b"\x1b");

    state.handle_input_bytes(b"/");
    state.handle_input_bytes(b"does-not-exist");
    let empty = state.compose(106, 30).expect("empty help search");
    let text = empty
        .cells
        .chunks(empty.width as usize)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("no matching keybinds"));

    state.handle_input_bytes(b"\x1b");
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Help(ClientHelpOverlay {
            search_focused: false,
            ref query,
            scroll: 0,
        })) if query.is_empty()
    ));
    state.compose(106, 30).expect("restored help");
    state.handle_input_bytes(b"\x1b[F");
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Help(ClientHelpOverlay { scroll, .. }))
            if scroll == state.hits.help_max_scroll
    ));
    state.handle_input_bytes(b"\x1b[H");
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Help(ClientHelpOverlay {
            scroll: 0,
            ..
        }))
    ));
    state.handle_input_bytes(b"?");
    assert!(state.overlay.is_none());
}

#[test]
fn resize_mode_reuses_endpoint_resize_and_stays_active_until_done() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());

    assert!(state.handle_input_bytes(&[0x02]).actions.is_empty());
    assert!(state.handle_input_bytes(b"r").actions.is_empty());
    assert_eq!(state.mode, ClientShellMode::Resize);

    let modified = state.handle_input_bytes(b"\x1b[1;2D");
    assert!(matches!(
        &modified.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(
                &request.method,
                crate::api::schema::Method::PaneResize(params)
                    if params.direction == crate::api::schema::PaneDirection::Left
            )
    ));
    assert_eq!(state.mode, ClientShellMode::Resize);

    let resize = state.handle_input_bytes(b"h");
    let [ClientShellAction::Endpoint { request, .. }] = &resize.actions[..] else {
        panic!("resize should use endpoint API");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneResize(params)
            if params.pane_id.as_deref() == Some("pane_1")
                && params.direction == crate::api::schema::PaneDirection::Left
    ));
    assert_eq!(state.mode, ClientShellMode::Resize);

    assert!(state.handle_input_bytes(b"\r").actions.is_empty());
    assert_eq!(state.mode, ClientShellMode::Terminal);
}
