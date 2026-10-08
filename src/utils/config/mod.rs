mod load;
mod model;
mod sound;
pub(crate) mod ui;

pub use self::{
    load::{config_diagnostic_summary, config_dir, load_live_config, state_dir},
    model::{
        AgentPanelSortConfig, Config, ConfigReloadReport, ConfigReloadStatus, HostCursorModeConfig,
        NewTerminalCwdConfig, PaneBordersConfig, ShellModeConfig, ToastClipboardPosition,
        ToastConfig, ToastDelivery, ToastHerdrPosition, MAX_TOAST_DELAY_SECONDS,
    },
    sound::SoundConfig,
    ui::{
        theme::{parse_color, CustomThemeColors, ModeThemeColors, ThemeConfig},
        window_title::{WindowTitlePart, WindowTitleTemplate, WindowTitleToken},
    },
};

pub(crate) use self::ui::keys::parse_key_combo;
pub(crate) use self::ui::{
    theme::canonical_theme_name,
    window_title::{sanitize_window_title_text, window_title_diagnostics},
};
#[cfg(test)]
pub(crate) use self::{load::config_path, ui::theme::THEME_NAMES};

pub const CONFIG_PATH_ENV_VAR: &str = "HERDR_CONFIG_PATH";

pub const DEFAULT_SCROLLBACK_LIMIT_BYTES: usize = 10_000_000;
pub const DEFAULT_MOUSE_SCROLL_LINES: usize = 3;
pub const DEFAULT_MOBILE_WIDTH_THRESHOLD: u16 = 64;
pub const DEFAULT_HEADLESS_COLS: u16 = 120;
pub const DEFAULT_HEADLESS_ROWS: u16 = 40;

#[cfg(test)]
pub(crate) fn app_dir_name() -> &'static str {
    load::app_dir_name()
}

#[cfg(test)]
pub(crate) fn test_config_env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[cfg(test)]
pub(crate) struct TestBusEnvGuard<'a> {
    _lock: &'a std::sync::MutexGuard<'static, ()>,
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

#[cfg(test)]
// A running Bus exports these variables, which outrank test config/session fixtures.
// Borrow the shared lock so restoration always happens before it is released.
pub(crate) fn test_without_bus_env<'a>(
    lock: &'a std::sync::MutexGuard<'static, ()>,
) -> TestBusEnvGuard<'a> {
    let previous = ["BUS_DATA_DIR", "BUS_SESSION_ID"]
        .into_iter()
        .map(|key| {
            let previous = std::env::var_os(key);
            std::env::remove_var(key);
            (key, previous)
        })
        .collect();
    TestBusEnvGuard {
        _lock: lock,
        previous,
    }
}

#[cfg(test)]
impl Drop for TestBusEnvGuard<'_> {
    fn drop(&mut self) {
        for (key, value) in self.previous.drain(..) {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

impl Config {
    pub fn kitty_graphics_enabled(&self) -> bool {
        self.terminal.kitty_graphics.unwrap_or(true)
    }

    pub fn collect_diagnostics(&self) -> Vec<String> {
        self.theme
            .diagnostics()
            .into_iter()
            .chain(self.ui.sound.diagnostics())
            .chain(window_title_diagnostics(&self.ui.window_title))
            .chain(self.invalid_headless_size_diagnostic())
            .collect()
    }

    pub(crate) fn headless_size(&self) -> (u16, u16) {
        if self.invalid_headless_size_diagnostic().is_some() {
            (DEFAULT_HEADLESS_COLS, DEFAULT_HEADLESS_ROWS)
        } else {
            (self.server.headless_cols, self.server.headless_rows)
        }
    }

    pub(crate) fn invalid_headless_size_diagnostic(&self) -> Option<String> {
        (self.server.headless_cols == 0 || self.server.headless_rows == 0).then(|| {
            format!(
                "server.headless_cols and server.headless_rows must be greater than zero (got {}x{})",
                self.server.headless_cols, self.server.headless_rows
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_host_cursor_defaults_to_auto_and_parses_overrides() {
        let default_config = Config::default();
        assert_eq!(default_config.ui.host_cursor, HostCursorModeConfig::Auto);

        let native: Config = toml::from_str("[ui]\nhost_cursor = 'native'\n").unwrap();
        assert_eq!(native.ui.host_cursor, HostCursorModeConfig::Native);

        let drawn: Config = toml::from_str("[ui]\nhost_cursor = 'drawn'\n").unwrap();
        assert_eq!(drawn.ui.host_cursor, HostCursorModeConfig::Drawn);
    }
}
