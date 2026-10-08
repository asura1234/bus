use super::{Palette, ThemeRuntimeConfig};
use crate::config::Config;

fn normalize_theme_name(name: &str) -> String {
    name.to_lowercase().replace([' ', '_'], "-")
}

fn sibling_theme_names(name: &str) -> (String, String) {
    match normalize_theme_name(name).as_str() {
        "catppuccin" | "catppuccin-mocha" | "catppuccin-latte" | "latte" | "light" => {
            ("catppuccin".to_string(), "catppuccin-latte".to_string())
        }
        "tokyo-night" | "tokyonight" | "tokyo-night-day" | "tokyo-day" | "tokyonight-day" => {
            ("tokyo-night".to_string(), "tokyo-night-day".to_string())
        }
        "gruvbox" | "gruvbox-dark" | "gruvbox-light" => {
            ("gruvbox".to_string(), "gruvbox-light".to_string())
        }
        "one-dark" | "onedark" | "one-light" | "onelight" => {
            ("one-dark".to_string(), "one-light".to_string())
        }
        "solarized" | "solarized-dark" | "solarized-light" => {
            ("solarized".to_string(), "solarized-light".to_string())
        }
        "kanagawa" | "kanagawa-lotus" | "lotus" => {
            ("kanagawa".to_string(), "kanagawa-lotus".to_string())
        }
        "rose-pine" | "rosepine" | "rose-pine-dawn" | "rosepine-dawn" | "dawn" => {
            ("rose-pine".to_string(), "rose-pine-dawn".to_string())
        }
        _ => (name.to_string(), name.to_string()),
    }
}

pub(crate) fn theme_runtime_config(
    config: &crate::config::Config,
    use_legacy_ui_accent: bool,
) -> ThemeRuntimeConfig {
    let manual_name = config
        .theme
        .name
        .clone()
        .unwrap_or_else(|| "catppuccin".to_string());
    let (default_dark, default_light) = sibling_theme_names(&manual_name);
    ThemeRuntimeConfig {
        manual_name,
        dark_name: config.theme.dark_name.clone().unwrap_or(default_dark),
        light_name: config.theme.light_name.clone().unwrap_or(default_light),
        auto_switch: config.theme.auto_switch,
        custom: config.theme.custom.clone(),
        legacy_accent: (use_legacy_ui_accent
            && config.ui.accent != "cyan"
            && config
                .theme
                .custom
                .as_ref()
                .and_then(|c| c.accent.as_ref())
                .is_none())
        .then(|| config.ui.accent.clone()),
    }
}

fn resolve_palette_for_theme_name(
    name: &str,
    fallback_name: &str,
    runtime: &ThemeRuntimeConfig,
    mode_custom: Option<&crate::config::ModeThemeColors>,
) -> Palette {
    let mut palette = Palette::from_name(name).unwrap_or_else(|| {
        tracing::warn!(target: "bus::server::app", theme = name,
            fallback = fallback_name,
            "unknown theme, falling back"
        );
        Palette::from_name(fallback_name).unwrap_or_else(Palette::catppuccin)
    });

    if let Some(custom) = &runtime.custom {
        palette = palette.with_overrides(custom);
    }
    if let Some(accent) = &runtime.legacy_accent {
        palette.accent = crate::config::parse_color(accent);
    }
    if let Some(custom) = mode_custom {
        palette = palette.with_mode_overrides(custom);
    }

    palette
}

pub(crate) fn resolve_effective_theme(
    runtime: &ThemeRuntimeConfig,
    appearance: Option<crate::terminal_theme::HostAppearance>,
) -> (Palette, String) {
    let (name, fallback, mode_custom) = if runtime.auto_switch {
        match appearance.unwrap_or(crate::terminal_theme::HostAppearance::Dark) {
            crate::terminal_theme::HostAppearance::Dark => (
                &runtime.dark_name,
                "catppuccin",
                runtime
                    .custom
                    .as_ref()
                    .and_then(|custom| custom.dark.as_ref()),
            ),
            crate::terminal_theme::HostAppearance::Light => (
                &runtime.light_name,
                "catppuccin-latte",
                runtime
                    .custom
                    .as_ref()
                    .and_then(|custom| custom.light.as_ref()),
            ),
        }
    } else {
        (&runtime.manual_name, "catppuccin", None)
    };
    (
        resolve_palette_for_theme_name(name, fallback, runtime, mode_custom),
        name.clone(),
    )
}

pub(crate) fn client_theme_runtime_from_config(config: &Config) -> ThemeRuntimeConfig {
    theme_runtime_config(config, true)
}

pub(crate) fn client_palette_from_config(config: &Config) -> Palette {
    let runtime = client_theme_runtime_from_config(config);
    resolve_effective_theme(&runtime, None).0
}

pub(crate) fn client_palette_for_appearance(
    runtime: &ThemeRuntimeConfig,
    appearance: crate::terminal_theme::HostAppearance,
) -> Palette {
    resolve_effective_theme(runtime, Some(appearance)).0
}
