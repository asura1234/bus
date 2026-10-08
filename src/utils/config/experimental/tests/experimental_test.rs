#[test]
fn reveal_hidden_cursor_for_cjk_ime_default_off_and_parse() {
    let default_config = Config::default();
    assert!(!default_config.experimental.reveal_hidden_cursor_for_cjk_ime);

    let toml = r#"
[experimental]
reveal_hidden_cursor_for_cjk_ime = true
"#;
    let config: Config = toml::from_str(toml).unwrap();
    assert!(config.experimental.reveal_hidden_cursor_for_cjk_ime);
}

#[test]
fn switch_ascii_input_source_in_prefix_default_off_and_parse() {
    let default_config = Config::default();
    assert!(
        !default_config
            .experimental
            .switch_ascii_input_source_in_prefix
    );

    let toml = r#"
[experimental]
switch_ascii_input_source_in_prefix = true
"#;
    let config: Config = toml::from_str(toml).unwrap();
    assert!(config.experimental.switch_ascii_input_source_in_prefix);
}

#[test]
fn cjk_ime_cursor_shape_default_steady_block_and_parse() {
    let default_config = Config::default();
    assert_eq!(
        default_config.experimental.cjk_ime_cursor_shape,
        ImeCursorShape::SteadyBlock
    );

    let toml = r#"
[experimental]
cjk_ime_cursor_shape = "bar"
"#;
    let config: Config = toml::from_str(toml).unwrap();
    assert_eq!(
        config.experimental.cjk_ime_cursor_shape,
        ImeCursorShape::Bar
    );
}

#[test]
fn cjk_ime_agents_default_empty_and_parse() {
    let default_config = Config::default();
    assert!(default_config.experimental.cjk_ime_agents.is_empty());

    let toml = r#"
[experimental]
cjk_ime_agents = ["claude", "codex"]
"#;
    let config: Config = toml::from_str(toml).unwrap();
    assert_eq!(
        config.experimental.cjk_ime_agents,
        vec!["claude".to_string(), "codex".to_string()]
    );
}

#[test]
fn pane_history_persistence_is_opt_in() {
    assert!(!Config::default().experimental.pane_history);

    let toml = r#"
[experimental]
pane_history = true
"#;
    let config: Config = toml::from_str(toml).unwrap();

    assert!(config.experimental.pane_history);
}

#[test]
fn experimental_config_parses() {
    let toml = r#"
[experimental]
pane_history = true
switch_ascii_input_source_in_prefix = true
"#;
    let config: Config = toml::from_str(toml).unwrap();
    assert!(config.experimental.pane_history);
    assert!(config.experimental.switch_ascii_input_source_in_prefix);
}
