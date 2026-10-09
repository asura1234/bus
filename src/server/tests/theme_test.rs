use super::*;

#[test]
fn theme_auto_switch_is_opt_in_and_preserves_manual_default() {
    let mut config = Config::default();
    config.theme.name = Some("tokyo-night".to_string());
    config.theme.custom = Some(crate::utils::config::CustomThemeColors {
        light: Some(crate::utils::config::ModeThemeColors {
            accent: Some("#010203".to_string()),
            ..Default::default()
        }),
        dark: Some(crate::utils::config::ModeThemeColors {
            accent: Some("#040506".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    });
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();

    let app = App::new(
        &config,
        crate::server::app::AppPolicy::TEST,
        None,
        api_rx,
        crate::server::api::EventHub::default(),
    );

    assert!(!app.state.theme_runtime.auto_switch);
    assert_eq!(app.state.theme_name, "tokyo-night");
    assert_eq!(
        app.state.palette,
        crate::utils::theme::Palette::tokyo_night()
    );
}

#[test]
fn theme_auto_switch_uses_sibling_map_and_explicit_appearance() {
    let mut config = Config::default();
    config.theme.name = Some("tokyo-night".to_string());
    config.theme.auto_switch = true;
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(
        &config,
        crate::server::app::AppPolicy::TEST,
        None,
        api_rx,
        crate::server::api::EventHub::default(),
    );

    assert_eq!(app.state.theme_name, "tokyo-night");
    assert!(app.set_host_terminal_appearance_state(
        Some(crate::utils::theme::color::HostAppearance::Light),
        true,
    ));

    assert_eq!(app.state.theme_name, "tokyo-night-day");
    assert_eq!(
        app.state.palette,
        crate::utils::theme::Palette::tokyo_night_day()
    );
}

#[test]
fn theme_auto_switch_applies_custom_overrides_after_active_base() {
    let mut config = Config::default();
    config.theme.name = Some("gruvbox".to_string());
    config.theme.auto_switch = true;
    config.theme.custom = Some(crate::utils::config::CustomThemeColors {
        accent: Some("#010203".to_string()),
        ..Default::default()
    });
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(
        &config,
        crate::server::app::AppPolicy::TEST,
        None,
        api_rx,
        crate::server::api::EventHub::default(),
    );

    app.set_host_terminal_appearance_state(
        Some(crate::utils::theme::color::HostAppearance::Light),
        true,
    );

    assert_eq!(app.state.theme_name, "gruvbox-light");
    assert_eq!(
        app.state.palette.accent,
        ratatui::style::Color::Rgb(1, 2, 3)
    );
}

#[test]
fn theme_auto_switch_layers_active_mode_overrides_last() {
    let mut config = Config::default();
    config.theme.name = Some("gruvbox".to_string());
    config.theme.auto_switch = true;
    config.theme.custom = Some(crate::utils::config::CustomThemeColors {
        accent: Some("#010203".to_string()),
        text: Some("#040506".to_string()),
        light: Some(crate::utils::config::ModeThemeColors {
            accent: Some("#070809".to_string()),
            ..Default::default()
        }),
        dark: Some(crate::utils::config::ModeThemeColors {
            text: Some("#0a0b0c".to_string()),
            sidebar_bg: Some("#0d0e0f".to_string()),
            active_row_bg: Some("#101112".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    });
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(
        &config,
        crate::server::app::AppPolicy::TEST,
        None,
        api_rx,
        crate::server::api::EventHub::default(),
    );

    assert_eq!(
        app.state.palette.accent,
        ratatui::style::Color::Rgb(1, 2, 3)
    );
    assert_eq!(
        app.state.palette.text,
        ratatui::style::Color::Rgb(10, 11, 12)
    );
    assert_eq!(
        app.state.palette.sidebar_bg,
        ratatui::style::Color::Rgb(13, 14, 15)
    );
    assert_eq!(
        app.state.palette.active_row_bg,
        ratatui::style::Color::Rgb(16, 17, 18)
    );

    app.set_host_terminal_appearance_state(
        Some(crate::utils::theme::color::HostAppearance::Light),
        true,
    );

    assert_eq!(
        app.state.palette.accent,
        ratatui::style::Color::Rgb(7, 8, 9)
    );
    assert_eq!(app.state.palette.text, ratatui::style::Color::Rgb(4, 5, 6));
}
