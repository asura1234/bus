use super::*;

#[test]
fn leaf_config_load_keeps_caller_overrides_outside_the_loader() {
    let guard = crate::config::test_config_env_lock().lock().unwrap();
    let _bus = crate::config::test_without_bus_env(&guard);
    let root = std::env::temp_dir().join(format!("herdr-config-leaf-{}", std::process::id()));
    std::fs::create_dir_all(root.join("herdr-config")).unwrap();
    std::fs::write(
        root.join("herdr-config/config.toml"),
        "onboarding = true\n[ui.sound]\nenabled = true\n",
    )
    .unwrap();
    std::env::set_var("BUS_DATA_DIR", &root);
    let loaded = Config::load();
    assert!(loaded.diagnostics.is_empty());
    assert_eq!(loaded.config.onboarding, Some(true));
    assert!(loaded.config.ui.sound.enabled);
    assert_eq!(config_dir(), root.join("herdr-config"));
    assert_eq!(state_dir(), root.join("herdr-state"));
    std::env::remove_var("BUS_DATA_DIR");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn config_diagnostic_summary_uses_compact_actionable_banner() {
    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    let diagnostics = vec![
        "one".to_string(),
        "two".to_string(),
        "three".to_string(),
        "four".to_string(),
        "five".to_string(),
    ];

    assert_eq!(
        config_diagnostic_summary(&diagnostics).as_deref(),
        Some("config.toml; herdr config check")
    );
}

#[test]
fn config_diagnostic_summary_reports_unknown_keys_as_invalid() {
    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    let startup = vec![
        "unknown config key ui.mouse_captur; using defaults".to_string(),
        "unknown config key keys.new_tabb; using defaults".to_string(),
    ];
    assert_eq!(
        config_diagnostic_summary(&startup).as_deref(),
        Some("config.toml invalid; using defaults; herdr config check")
    );

    let reload = vec!["unknown config key ui.mouse_captur; keeping current config".to_string()];
    assert_eq!(
        config_diagnostic_summary(&reload).as_deref(),
        Some("config.toml invalid; keeping current config; herdr config check")
    );
}

#[test]
fn config_diagnostic_summary_keeps_mixed_diagnostics_generic() {
    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    let diagnostics = vec![
        "invalid ui config: invalid type: string; keeping current ui settings".to_string(),
        "theme.name = \"catppucin\" is not a known theme".to_string(),
    ];

    assert_eq!(
        config_diagnostic_summary(&diagnostics).as_deref(),
        Some("config.toml; herdr config check")
    );
}

#[test]
fn config_diagnostic_summary_reports_default_fallback() {
    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    let diagnostics = vec![
            "config parse error: TOML parse error at line 33, column 8\n   |\n33 | type = \"popup\"\n   |        ^^^^^^^\nunknown variant `popup`; using defaults"
                .to_string(),
        ];

    assert_eq!(
        config_diagnostic_summary(&diagnostics).as_deref(),
        Some("config.toml invalid; using defaults; herdr config check")
    );
}

#[test]
fn config_diagnostic_summary_reports_unreadable_config_impact() {
    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    let startup = vec!["config read error: permission denied; using defaults".to_string()];
    assert_eq!(
        config_diagnostic_summary(&startup).as_deref(),
        Some("config.toml unreadable; using defaults; herdr config check")
    );

    let reload = vec!["config read error: permission denied; keeping current config".to_string()];
    assert_eq!(
        config_diagnostic_summary(&reload).as_deref(),
        Some("config.toml unreadable; keeping current config; herdr config check")
    );
}

#[test]
fn config_diagnostic_summary_reports_retained_live_config() {
    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    let diagnostics = vec![
        "config parse error: TOML parse error at line 7, column 4; keeping current config"
            .to_string(),
    ];

    assert_eq!(
        config_diagnostic_summary(&diagnostics).as_deref(),
        Some("config.toml invalid; keeping current config; herdr config check")
    );
}

#[test]
fn config_loaders_report_unreadable_path() {
    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    let _bus = crate::config::test_without_bus_env(&_guard);
    let path = std::env::temp_dir().join(format!("herdr-config-unreadable-{}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    std::env::set_var(CONFIG_PATH_ENV_VAR, &path);

    let startup = Config::load();
    assert!(startup
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.contains("config read error")
            && diagnostic.contains("using defaults")));

    let reload = load_live_config().unwrap_err();
    assert!(reload.iter().any(|diagnostic| {
        diagnostic.contains("config read error") && diagnostic.contains("keeping current config")
    }));

    std::env::remove_var(CONFIG_PATH_ENV_VAR);
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn load_live_config_parses_session_section() {
    let loaded = load_live_config_from_str(
        r#"
[session]
resume_agents_on_restore = true
"#,
    )
    .unwrap();

    assert!(loaded.config.session.resume_agents_on_restore);
    assert!(loaded.diagnostics.is_empty());
    assert!(loaded.invalid_sections.is_empty());
}

#[test]
fn load_live_config_warns_about_unknown_theme_names() {
    let loaded = load_live_config_from_str(
        r#"
[theme]
name = "catppucin"
"#,
    )
    .unwrap();

    assert_eq!(loaded.diagnostics.len(), 1);
    assert!(loaded.diagnostics[0].contains("theme.name = \"catppucin\""));
}

#[test]
fn load_live_config_rejects_unknown_top_level_sections() {
    let rejected = load_live_config_from_str(
        r#"
[toast]
delivery = "system"

[ui.toast]
delivery = "herdr"
"#,
    )
    .unwrap_err();

    assert_eq!(
        rejected,
        vec!["unknown config section [toast] (did you mean [ui.toast]?); keeping current config"]
    );
}

#[test]
fn load_live_config_rejects_every_unknown_key() {
    let rejected = load_live_config_from_str(
        r##"
plugin = []

[theme.custom]
accentt = "#ffffff"

[advanced]
scrollback_limit_bytes = 42

[ui]
mouse_capture = false
mouse_captur = true
"foo.bar" = true
"foo.?.bar" = false

[ui.toast]
delivery = "herdr"
delivry = "system"
"##,
    )
    .unwrap_err();

    assert_eq!(
        rejected,
        vec![
            "unknown config key plugin; keeping current config",
            "unknown config key theme.custom.accentt; keeping current config",
            "unknown config key ui.\"foo.?.bar\"; keeping current config",
            "unknown config key ui.\"foo.bar\"; keeping current config",
            "unknown config key ui.mouse_captur; keeping current config",
            "unknown config key ui.toast.delivry; keeping current config",
        ]
    );
}

#[test]
fn load_live_config_rejects_retired_keys() {
    let rejected = load_live_config_from_str(
        r#"
[ui]
agent_panel_scope = "current"
status_indicators = "symbols"
agent_panel_sort = "priority"
sidebar_width = 30

[ui.sidebar.agents]
rows = [["agent"]]

[advanced]
scrollback_lines = 100

[experimental]
kitty_graphics = false
"#,
    )
    .unwrap_err();

    assert_eq!(
        rejected,
        vec![
            "unknown config key ui.agent_panel_scope; keeping current config",
            "unknown config key ui.sidebar; keeping current config",
            "unknown config key ui.sidebar_width; keeping current config",
            "unknown config key ui.status_indicators; keeping current config",
            "unknown config key advanced.scrollback_lines; keeping current config",
            "unknown config key experimental.kitty_graphics; keeping current config",
        ]
    );
}

#[test]
fn load_live_config_discards_ignored_keys_from_an_invalid_section() {
    let loaded = load_live_config_from_str(
        r#"
[ui]
mouse_capture = "yes"
mouse_captur = true
"#,
    )
    .unwrap();

    assert_eq!(loaded.diagnostics.len(), 1);
    assert!(loaded.diagnostics[0].contains("invalid ui config"));
    assert!(!loaded.diagnostics[0].starts_with("unknown config key"));
    assert_eq!(loaded.invalid_sections, vec!["ui"]);
}

#[test]
fn startup_config_falls_back_to_defaults_on_a_retired_key() {
    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    let _bus = crate::config::test_without_bus_env(&_guard);
    let path = std::env::temp_dir().join(format!(
        "herdr-config-retired-agent-panel-scope-{}.toml",
        std::process::id()
    ));
    std::fs::write(
        &path,
        "[ui]\nagent_panel_scope = \"all\"\nagent_panel_sort = \"priority\"\n",
    )
    .unwrap();
    std::env::set_var(CONFIG_PATH_ENV_VAR, &path);

    let loaded = Config::load();

    std::env::remove_var(CONFIG_PATH_ENV_VAR);
    let _ = std::fs::remove_file(path);

    assert_eq!(
        loaded.diagnostics,
        vec!["unknown config key ui.agent_panel_scope; using defaults"]
    );
    assert_eq!(
        loaded.config.ui.agent_panel_sort,
        Config::default().ui.agent_panel_sort
    );
}

#[test]
fn startup_config_falls_back_to_defaults_on_an_unknown_section() {
    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    let _bus = crate::config::test_without_bus_env(&_guard);
    let path = std::env::temp_dir().join(format!(
        "herdr-config-unknown-section-{}.toml",
        std::process::id()
    ));
    std::fs::write(
        &path,
        r#"
[[plugin]]
id = "example"

[ui.toast]
delivery = "system"
"#,
    )
    .unwrap();
    std::env::set_var(CONFIG_PATH_ENV_VAR, &path);

    let loaded = Config::load();

    assert_eq!(
        loaded.diagnostics,
        vec!["unknown config section [[plugin]]; using defaults"]
    );
    assert_eq!(
        loaded.config.ui.toast.delivery,
        Config::default().ui.toast.delivery
    );

    std::env::remove_var(CONFIG_PATH_ENV_VAR);
    let _ = std::fs::remove_file(path);
}
