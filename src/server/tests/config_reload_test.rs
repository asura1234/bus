use super::*;

#[test]
fn reload_config_updates_live_state() {
    let _guard = config_env_lock().lock().unwrap();
    let _bus = crate::config::test_without_bus_env(&_guard);
    let path = temp_config_path("reload-config-success");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
            &path,
            "[terminal]\ndefault_shell = \"nu\"\nshell_mode = \"non_login\"\nnew_cwd = \"home\"\n[server]\nheadless_cols = 160\nheadless_rows = 50\n[ui]\nagent_panel_sort = \"priority\"\n[ui.toast]\ndelivery = \"herdr\"\n",
        )
        .unwrap();
    std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

    let mut app = test_app();
    let report = app.reload_config();

    assert_eq!(report.status, crate::config::ConfigReloadStatus::Applied);
    assert_eq!(app.state.headless_size, (160, 50));
    assert_eq!(
        app.state.toast_config.delivery,
        crate::config::ToastDelivery::Herdr
    );
    assert_eq!(app.state.agent_panel_sort, state::AgentPanelSort::Priority);
    let report = app.reload_config();
    assert_eq!(report.status, crate::config::ConfigReloadStatus::Applied);
    assert!(app.state.request_client_config_reload);
    assert_eq!(app.state.default_shell, "nu");
    assert_eq!(
        app.state.shell_mode,
        crate::config::ShellModeConfig::NonLogin
    );
    assert_eq!(
        app.state.new_terminal_cwd,
        crate::config::NewTerminalCwdConfig::Home
    );
    assert!(app.state.config_diagnostic.is_none());
    let toast = app.state.toast.as_ref().unwrap();
    assert_eq!(toast.kind, crate::app::state::ToastKind::UpdateInstalled);
    assert_eq!(toast.title, "reloaded config");
    assert_eq!(toast.context, "using config.toml");

    std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn reload_config_keeps_kitty_graphics_until_restart() {
    let _guard = config_env_lock().lock().unwrap();
    let _bus = crate::config::test_without_bus_env(&_guard);
    let path = temp_config_path("reload-config-kitty-graphics");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "[terminal]\nkitty_graphics = false\n").unwrap();
    std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

    let mut app = test_app();
    assert!(app.state.kitty_graphics_enabled);

    let report = app.reload_config();

    assert_eq!(report.status, crate::config::ConfigReloadStatus::Partial);
    assert!(app.state.kitty_graphics_enabled);
    assert_eq!(
        report.diagnostics,
        vec![
            "terminal.kitty_graphics changes require restarting Herdr; kept current setting"
                .to_owned()
        ]
    );

    std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn reload_config_requests_client_reload_for_host_cursor_only_change() {
    let _guard = config_env_lock().lock().unwrap();
    let _bus = crate::config::test_without_bus_env(&_guard);
    let path = temp_config_path("reload-config-host-cursor");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "[ui]\nhost_cursor = \"native\"\n").unwrap();
    std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

    let mut app = test_app();
    app.state.request_client_config_reload = false;

    let report = app.reload_config();

    assert_eq!(report.status, crate::config::ConfigReloadStatus::Applied);
    assert_eq!(
        app.loaded_host_cursor,
        crate::config::HostCursorModeConfig::Native
    );
    assert!(app.state.request_client_config_reload);

    std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn reload_config_rejects_an_unknown_key_and_keeps_the_current_config() {
    let _guard = config_env_lock().lock().unwrap();
    let _bus = crate::config::test_without_bus_env(&_guard);
    let path = temp_config_path("reload-config-unknown-key");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

    let mut app = test_app();
    let original_pane_borders = app.state.pane_borders;
    std::fs::write(
        &path,
        "[ui]\npane_borders = \"always\"\nmouse_captur = false\n",
    )
    .unwrap();

    let report = app.reload_config();

    assert_eq!(report.status, crate::config::ConfigReloadStatus::Failed);
    assert_eq!(
        report.diagnostics,
        vec!["unknown config key ui.mouse_captur; keeping current config"]
    );
    assert_eq!(app.state.pane_borders, original_pane_borders);
    assert_eq!(
        app.state.config_diagnostic.as_deref(),
        Some("config.toml invalid; keeping current config; herdr config check")
    );

    std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn reload_config_preserves_invalid_terminal_section_but_applies_valid_ui() {
    let _guard = config_env_lock().lock().unwrap();
    let _bus = crate::config::test_without_bus_env(&_guard);
    let path = temp_config_path("reload-config-invalid-terminal-section");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
            &path,
            "[terminal]\ndefault_shell = \"nu\"\nshell_mode = \"sideways\"\nnew_cwd = \"home\"\n[ui.toast]\ndelivery = \"terminal\"\n",
        )
        .unwrap();
    std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

    let mut app = test_app();
    let original_default_shell = app.state.default_shell.clone();
    let original_shell_mode = app.state.shell_mode;
    let original_new_cwd = app.state.new_terminal_cwd.clone();
    let report = app.reload_config();

    assert_eq!(report.status, crate::config::ConfigReloadStatus::Partial);
    assert!(report
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.contains("invalid terminal config")));
    assert_eq!(app.state.default_shell, original_default_shell);
    assert_eq!(app.state.shell_mode, original_shell_mode);
    assert_eq!(app.state.new_terminal_cwd, original_new_cwd);
    assert_eq!(
        app.state.toast_config.delivery,
        crate::config::ToastDelivery::Terminal
    );
    std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}
