#[test]
fn terminal_default_shell_defaults_empty_and_parses() {
    let default_config = Config::default();
    assert!(default_config.terminal.default_shell.is_empty());
    assert_eq!(default_config.terminal.shell_mode, ShellModeConfig::Auto);

    let toml = r#"
[terminal]
default_shell = "nu"
shell_mode = "non_login"
"#;
    let config: Config = toml::from_str(toml).unwrap();
    assert_eq!(config.terminal.default_shell, "nu");
    assert_eq!(config.terminal.shell_mode, ShellModeConfig::NonLogin);
}

#[test]
fn terminal_new_cwd_defaults_follow_and_parses() {
    let default_config = Config::default();
    assert_eq!(
        default_config.terminal.new_cwd,
        NewTerminalCwdConfig::Follow
    );

    let config: Config = toml::from_str(
        r#"
[terminal]
new_cwd = "home"
"#,
    )
    .unwrap();
    assert_eq!(config.terminal.new_cwd, NewTerminalCwdConfig::Home);

    let config: Config = toml::from_str(
        r#"
[terminal]
new_cwd = "~/Projects"
"#,
    )
    .unwrap();
    assert_eq!(
        config.terminal.new_cwd,
        NewTerminalCwdConfig::Path("~/Projects".into())
    );
}

#[test]
fn kitty_graphics_default_on_with_stable_opt_out() {
    assert!(Config::default().kitty_graphics_enabled());

    let config: Config = toml::from_str(
        r#"
[terminal]
kitty_graphics = false
"#,
    )
    .unwrap();
    assert!(!config.kitty_graphics_enabled());
}
