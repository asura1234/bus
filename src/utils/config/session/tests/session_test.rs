#[test]
fn resume_agents_on_restore_defaults_on_and_parses() {
    let default_config = Config::default();
    assert!(default_config.session.resume_agents_on_restore);

    let toml = r#"
[session]
resume_agents_on_restore = false
"#;
    let config: Config = toml::from_str(toml).unwrap();
    assert!(!config.session.resume_agents_on_restore);
}
