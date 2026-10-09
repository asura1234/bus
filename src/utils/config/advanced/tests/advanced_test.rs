#[test]
fn advanced_defaults_include_scrollback_limit_bytes() {
    let config = Config::default();
    assert_eq!(
        config.advanced.scrollback_limit_bytes,
        DEFAULT_SCROLLBACK_LIMIT_BYTES
    );
}

#[test]
fn advanced_config_parses() {
    let toml = r#"
[advanced]
scrollback_limit_bytes = 12345
"#;
    let config: Config = toml::from_str(toml).unwrap();
    assert_eq!(config.advanced.scrollback_limit_bytes, 12345);
}
