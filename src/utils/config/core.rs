use serde::{Deserialize, Serialize};

pub use super::advanced::AdvancedConfig;
pub use super::experimental::ExperimentalConfig;
#[cfg(test)]
use super::experimental::ImeCursorShape;
#[cfg(test)]
use super::interface::TabBarPositionConfig;
pub use super::interface::{
    AgentPanelSortConfig, HostCursorModeConfig, PaneBordersConfig, UiConfig,
};
pub use super::server::ServerConfig;
pub use super::session::SessionConfig;
pub use super::terminal::{NewTerminalCwdConfig, ShellModeConfig, TerminalConfig};
pub use super::toast::{
    ToastBusPosition, ToastClipboardPosition, ToastConfig, ToastDelivery, MAX_TOAST_DELAY_SECONDS,
};
use super::ThemeConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConfigReloadStatus {
    Applied,
    Partial,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ConfigReloadReport {
    pub status: ConfigReloadStatus,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub onboarding: Option<bool>,
    pub theme: ThemeConfig,
    pub terminal: TerminalConfig,
    pub session: SessionConfig,
    pub server: ServerConfig,
    pub ui: UiConfig,
    pub advanced: AdvancedConfig,
    pub experimental: ExperimentalConfig,
}

#[derive(Debug)]
pub struct LoadedConfig {
    pub config: Config,
    pub diagnostics: Vec<String>,
    pub invalid_sections: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::super::{
        DEFAULT_MOBILE_WIDTH_THRESHOLD, DEFAULT_MOUSE_SCROLL_LINES, DEFAULT_SCROLLBACK_LIMIT_BYTES,
    };
    use crossterm::event::KeyModifiers;
    include!("tests/model_test.rs");
}
