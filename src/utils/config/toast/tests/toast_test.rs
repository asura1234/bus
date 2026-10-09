#[test]
fn toast_config_parses() {
    let toml = r#"
[ui.toast]
delivery = "terminal"
delay_seconds = 2

[ui.toast.clipboard]
enabled = false
position = "top-center"
"#;
    let config: Config = toml::from_str(toml).unwrap();
    assert_eq!(config.ui.toast.delivery, ToastDelivery::Terminal);
    assert_eq!(config.ui.toast.delay_seconds, 2);
    assert!(!config.ui.toast.clipboard.enabled);
    assert_eq!(
        config.ui.toast.clipboard.position,
        ToastClipboardPosition::TopCenter
    );
}

#[test]
fn toast_config_defaults_preserve_existing_behavior_with_delay() {
    let config = Config::default();
    assert_eq!(config.ui.toast.delivery, ToastDelivery::Off);
    assert_eq!(config.ui.toast.delay_seconds, 1);
    assert!(config.ui.toast.clipboard.enabled);
    assert_eq!(
        config.ui.toast.clipboard.position,
        ToastClipboardPosition::BottomCenter
    );
}

#[test]
fn toast_config_parses_system_delivery() {
    let toml = r#"
[ui.toast]
delivery = "system"
"#;
    let config: Config = toml::from_str(toml).unwrap();
    assert_eq!(config.ui.toast.delivery, ToastDelivery::System);
}

#[test]
fn toast_config_rejects_unbounded_delay() {
    let toml = format!(
        r#"
[ui.toast]
delay_seconds = {}
"#,
        MAX_TOAST_DELAY_SECONDS + 1
    );

    let error = toml::from_str::<Config>(&toml).unwrap_err().to_string();

    assert!(error.contains("ui.toast.delay_seconds must be between 0 and 3600"));
}
