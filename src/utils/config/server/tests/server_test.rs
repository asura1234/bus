#[test]
fn server_headless_size_defaults_and_parses() {
    let default_config = Config::default();
    assert_eq!(
        default_config.server.headless_cols,
        crate::config::DEFAULT_HEADLESS_COLS
    );
    assert_eq!(
        default_config.server.headless_rows,
        crate::config::DEFAULT_HEADLESS_ROWS
    );

    let config: Config = toml::from_str(
        r#"[server]
headless_cols = 160
headless_rows = 50
"#,
    )
    .unwrap();
    assert_eq!(config.server.headless_cols, 160);
    assert_eq!(config.server.headless_rows, 50);

    let invalid: Config = toml::from_str(
        r#"[server]
headless_cols = 0
headless_rows = 50
"#,
    )
    .unwrap();
    assert!(invalid.invalid_headless_size_diagnostic().is_some());
    assert_eq!(
        invalid.headless_size(),
        (
            crate::config::DEFAULT_HEADLESS_COLS,
            crate::config::DEFAULT_HEADLESS_ROWS
        )
    );
}
