use crate::utils::theme::Palette;
use ratatui::style::Color;

fn rgb_luminance(color: Color) -> f64 {
    let Color::Rgb(r, g, b) = color else {
        panic!("expected RGB color, got {color:?}");
    };
    let channel = |value: u8| {
        let value = f64::from(value) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

fn contrast_ratio(a: Color, b: Color) -> f64 {
    let (lighter, darker) = {
        let a = rgb_luminance(a);
        let b = rgb_luminance(b);
        (a.max(b), a.min(b))
    };
    (lighter + 0.05) / (darker + 0.05)
}

#[test]
fn built_in_theme_names_resolve() {
    for name in crate::utils::config::THEME_NAMES {
        assert!(
            Palette::from_name(name).is_some(),
            "theme should resolve: {name}"
        );
    }
}

#[test]
fn built_in_active_rows_remain_visible_with_matching_terminal_backgrounds() {
    for name in crate::utils::config::THEME_NAMES
        .iter()
        .copied()
        .filter(|name| *name != "terminal")
    {
        let palette = Palette::from_name(name).unwrap();
        let background_contrast = contrast_ratio(palette.panel_bg, palette.active_row_bg);
        assert!(
                background_contrast >= 1.05,
                "active row blends into the matching terminal background for {name}: {background_contrast:.2}:1"
            );

        let text_contrast = contrast_ratio(palette.text, palette.active_row_bg);
        assert!(
            text_contrast >= 3.0,
            "active row text loses contrast for {name}: {text_contrast:.2}:1"
        );
    }
}

#[test]
fn built_in_selection_rows_stay_distinct_from_background_and_active_rows() {
    for name in crate::utils::config::THEME_NAMES
        .iter()
        .copied()
        .filter(|name| *name != "terminal")
    {
        let palette = Palette::from_name(name).unwrap();
        let background_contrast = contrast_ratio(palette.panel_bg, palette.selection_bg);
        assert!(
                background_contrast >= 1.05,
                "selection row blends into the matching terminal background for {name}: {background_contrast:.2}:1"
            );

        let text_contrast = contrast_ratio(palette.text, palette.selection_bg);
        assert!(
            text_contrast >= 3.0,
            "selection row text loses contrast for {name}: {text_contrast:.2}:1"
        );
        assert_ne!(
            palette.selection_bg, palette.active_row_bg,
            "selection row shares the active row color for {name}"
        );
    }
}

#[test]
fn built_in_themes_leave_sidebar_background_unset() {
    for name in crate::utils::config::THEME_NAMES {
        let palette = Palette::from_name(name).unwrap();
        assert_eq!(
            palette.sidebar_bg,
            Color::Reset,
            "built-in theme changed the sidebar background: {name}"
        );
    }
}

#[test]
fn custom_sidebar_colors_override_the_defaults() {
    let custom = crate::utils::config::CustomThemeColors {
        sidebar_bg: Some("#181825".to_string()),
        active_row_bg: Some("#313244".to_string()),
        selection_bg: Some("#45475a".to_string()),
        ..Default::default()
    };
    let palette = Palette::catppuccin().with_overrides(&custom);

    assert_eq!(palette.sidebar_bg, Color::Rgb(24, 24, 37));
    assert_eq!(palette.active_row_bg, Color::Rgb(49, 50, 68));
    assert_eq!(palette.selection_bg, Color::Rgb(69, 71, 90));
}

#[test]
fn light_theme_aliases_resolve() {
    for name in ["light", "latte", "tokyo-day", "onelight", "lotus", "dawn"] {
        assert!(
            Palette::from_name(name).is_some(),
            "theme should resolve: {name}"
        );
    }
}
