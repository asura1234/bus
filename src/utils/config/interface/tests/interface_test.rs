#[test]
fn agent_panel_sort_config_parses_and_defaults() {
    assert_eq!(
        Config::default().ui.agent_panel_sort,
        AgentPanelSortConfig::Spaces
    );

    let toml = r#"
[ui]
agent_panel_sort = "priority"
"#;
    let config: Config = toml::from_str(toml).unwrap();
    assert_eq!(config.ui.agent_panel_sort, AgentPanelSortConfig::Priority);

    let retired = toml::from_str::<Config>("[ui]\nagent_panel_sort = \"workspaces\"");
    assert!(retired.is_err(), "the bus-era alias is gone");
}

#[test]
fn pane_borders_parse_modes_and_reject_booleans() {
    assert!(toml::from_str::<Config>("[ui]\npane_borders = true").is_err());

    let auto: Config = toml::from_str("[ui]\npane_borders = \"auto\"").unwrap();
    assert_eq!(auto.ui.pane_borders, PaneBordersConfig::Auto);

    let off: Config = toml::from_str("[ui]\npane_borders = \"off\"").unwrap();
    assert_eq!(off.ui.pane_borders, PaneBordersConfig::Off);

    assert!(toml::from_str::<Config>("[ui]\npane_borders = \"framed\"").is_err());
}

#[test]
fn pane_appearance_defaults_and_parse() {
    let default_config = Config::default();
    assert_eq!(default_config.ui.pane_borders, PaneBordersConfig::Auto);
    assert!(default_config.ui.pane_outer_borders);
    assert!(default_config.ui.pane_scrollbars);
    assert!(default_config.ui.pane_gaps);
    assert!(!default_config.ui.show_agent_labels_on_pane_borders);
    assert!(!default_config.ui.hide_tab_bar_when_single_tab);
    assert_eq!(
        default_config.ui.tab_bar_position,
        TabBarPositionConfig::Top
    );

    let toml = r#"
[ui]
pane_borders = "always"
pane_outer_borders = false
pane_scrollbars = false
pane_gaps = true
show_agent_labels_on_pane_borders = true
hide_tab_bar_when_single_tab = true
tab_bar_position = "bottom"
"#;
    let config: Config = toml::from_str(toml).unwrap();
    assert_eq!(config.ui.pane_borders, PaneBordersConfig::Always);
    assert!(!config.ui.pane_outer_borders);
    assert!(!config.ui.pane_scrollbars);
    assert!(config.ui.pane_gaps);
    assert!(config.ui.show_agent_labels_on_pane_borders);
    assert!(config.ui.hide_tab_bar_when_single_tab);
    assert_eq!(config.ui.tab_bar_position, TabBarPositionConfig::Bottom);
}

#[test]
fn prompt_new_tab_name_defaults_on_and_parses() {
    let default_config = Config::default();
    assert!(default_config.ui.prompt_new_tab_name);

    let toml = r#"
[ui]
prompt_new_tab_name = false
"#;
    let config: Config = toml::from_str(toml).unwrap();
    assert!(!config.ui.prompt_new_tab_name);
}

#[test]
fn prompt_new_workspace_name_defaults_off_and_parses() {
    let default_config = Config::default();
    assert!(!default_config.ui.prompt_new_workspace_name);

    let toml = r#"
[ui]
prompt_new_workspace_name = true
"#;
    let config: Config = toml::from_str(toml).unwrap();
    assert!(config.ui.prompt_new_workspace_name);
}

#[test]
fn mobile_width_threshold_default_and_parse() {
    let default_config = Config::default();
    assert_eq!(
        default_config.ui.mobile_width_threshold,
        DEFAULT_MOBILE_WIDTH_THRESHOLD
    );

    let toml = r#"
[ui]
mobile_width_threshold = 96
"#;
    let config: Config = toml::from_str(toml).unwrap();
    assert_eq!(config.ui.mobile_width_threshold, 96);
}

#[test]
fn mouse_capture_default_on_and_parse() {
    let default_config = Config::default();
    assert!(default_config.ui.mouse_capture);

    let toml = r#"
[ui]
mouse_capture = false
"#;
    let config: Config = toml::from_str(toml).unwrap();
    assert!(!config.ui.mouse_capture);
}

#[test]
fn copy_on_select_default_on_and_parse() {
    let default_config = Config::default();
    assert!(default_config.ui.copy_on_select);

    let toml = r#"
[ui]
copy_on_select = false
"#;
    let config: Config = toml::from_str(toml).unwrap();
    assert!(!config.ui.copy_on_select);
}

#[test]
fn right_click_passthrough_modifier_defaults_off_and_parses() {
    let default_config = Config::default();
    assert_eq!(default_config.ui.right_click_passthrough_modifiers(), None);

    for value in ["", "off", "none", "disabled"] {
        let toml = format!(
            r#"
[ui]
right_click_passthrough_modifier = "{value}"
"#
        );
        let config: Config = toml::from_str(&toml).unwrap();
        assert_eq!(
            config.ui.right_click_passthrough_modifiers(),
            None,
            "value {value:?} should disable passthrough"
        );
    }

    for (value, expected) in [
        ("ctrl", KeyModifiers::CONTROL),
        ("control", KeyModifiers::CONTROL),
        ("alt", KeyModifiers::ALT),
        ("option", KeyModifiers::ALT),
        ("cmd", KeyModifiers::SUPER),
        ("command", KeyModifiers::SUPER),
        ("super", KeyModifiers::SUPER),
        ("meta", KeyModifiers::META),
        ("hyper", KeyModifiers::HYPER),
    ] {
        let toml = format!(
            r#"
[ui]
right_click_passthrough_modifier = "{value}"
"#
        );
        let config: Config = toml::from_str(&toml).unwrap();
        assert_eq!(
            config.ui.right_click_passthrough_modifiers(),
            Some(expected),
            "value {value:?} should parse"
        );
    }

    let toml = r#"
[ui]
right_click_passthrough_modifier = "cmd+alt"
"#;
    let config: Config = toml::from_str(toml).unwrap();
    assert_eq!(
        config.ui.right_click_passthrough_modifiers(),
        Some(KeyModifiers::SUPER | KeyModifiers::ALT)
    );
}

#[test]
fn right_click_passthrough_modifier_rejects_shift() {
    for value in ["shift", "shift+ctrl", "ctrl+", "ctrl++alt", "banana"] {
        let toml = format!(
            r#"
[ui]
right_click_passthrough_modifier = "{value}"
"#
        );
        assert!(
            toml::from_str::<Config>(&toml).is_err(),
            "value {value:?} should be rejected"
        );
    }
}

#[test]
fn redraw_on_focus_gained_default_on_and_parse() {
    let default_config = Config::default();
    assert!(default_config.ui.redraw_on_focus_gained);

    let toml = r#"
[ui]
redraw_on_focus_gained = false
"#;
    let config: Config = toml::from_str(toml).unwrap();
    assert!(!config.ui.redraw_on_focus_gained);
}

#[test]
fn mouse_scroll_lines_defaults_to_three_and_parses() {
    let default_config = Config::default();
    assert_eq!(
        default_config.ui.mouse_scroll_lines(),
        DEFAULT_MOUSE_SCROLL_LINES
    );

    let toml = r#"
[ui]
mouse_scroll_lines = 1
"#;
    let config: Config = toml::from_str(toml).unwrap();
    assert_eq!(config.ui.mouse_scroll_lines(), 1);
}

#[test]
fn mouse_scroll_lines_rejects_zero() {
    let toml = r#"
[ui]
mouse_scroll_lines = 0
"#;
    assert!(toml::from_str::<Config>(toml).is_err());
}
