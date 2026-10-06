use super::*;

#[test]
fn startup_config_diagnostics_are_client_rendered_and_persist_until_replaced() {
    let config = ClientShellConfig::from_config(&Config::default())
        .with_startup_config_diagnostic(Some("local config warning".into()));
    let mut state = ClientShellState::new(config);
    let mut shared_snapshot = snapshot();
    shared_snapshot.config_diagnostic = Some("local config warning".into());
    state.set_snapshot(Box::new(shared_snapshot));
    assert_eq!(
        state.config_diagnostic.as_deref(),
        Some("client + endpoint: local config warning")
    );

    let mut endpoint_snapshot = snapshot();
    endpoint_snapshot.config_diagnostic = Some("endpoint config warning".into());
    state.set_snapshot(Box::new(endpoint_snapshot));
    state.set_pane_surface(surface());

    let frame = state.compose(106, 20).expect("diagnostic frame");
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
    assert!(text.contains("client: local config warning"));
    assert!(text.contains("endpoint: endpoint config warning"));

    state.handle_input_bytes(b"x");
    assert!(state.config_diagnostic.is_some());

    state.set_snapshot(Box::new(snapshot()));
    assert_eq!(
        state.config_diagnostic.as_deref(),
        Some("local config warning")
    );
}

#[test]
fn endpoint_reload_result_does_not_override_snapshot_diagnostic_authority() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let mut endpoint_snapshot = snapshot();
    endpoint_snapshot.config_diagnostic = Some("endpoint warning".into());
    state.set_snapshot(Box::new(endpoint_snapshot));
    state.pending_requests.insert(
        "reload-1".into(),
        PendingEndpointRequest {
            boot_id: "boot-1".into(),
            method_name: "server.reload_config".into(),
            confirmation_workspace_id: None,
            kind: PendingEndpointKind::ReloadConfig,
        },
    );

    state.handle_endpoint_result(
        "boot-1",
        "reload-1",
        Ok(crate::api::schema::ResponseResult::ConfigReload {
            status: crate::config::ConfigReloadStatus::Partial,
            diagnostics: vec!["keybinding warning".into()],
        }),
    );
    assert_eq!(state.config_diagnostic.as_deref(), Some("endpoint warning"));

    state.set_snapshot(Box::new(snapshot()));
    assert!(state.config_diagnostic.is_none());
}

#[test]
fn live_client_config_keeps_sound_diagnostics() {
    let mut shell_config = ClientShellConfig::from_config(&Config::default());
    let mut config = Config::default();
    config.ui.sound.path = Some(std::path::PathBuf::from("invalid.wav"));

    let diagnostics = shell_config.apply_live_config(&config, &[], &[]);
    assert!(diagnostics
        .iter()
        .any(|diagnostic| diagnostic.contains("expected an mp3 file")));
}
