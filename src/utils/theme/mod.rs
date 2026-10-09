//! Palette data, built-in colors, and runtime theme resolution.
mod builtin;
pub(crate) mod color;
mod palette;
mod resolve;

pub use palette::Palette;
pub(crate) use resolve::{
    client_palette_for_appearance, client_palette_from_config, client_theme_runtime_from_config,
    resolve_effective_theme, theme_runtime_config,
};

#[derive(Debug, Clone)]
pub struct ThemeRuntimeConfig {
    pub manual_name: String,
    pub dark_name: String,
    pub light_name: String,
    pub auto_switch: bool,
    pub custom: Option<crate::utils::config::CustomThemeColors>,
    pub legacy_accent: Option<String>,
}
