use super::{ClientShellConfig, ClientShellState, Config};
use crate::protocol::ClientSurfaceSize;

pub(super) fn merged_config_diagnostic(
    local: Option<&str>,
    endpoint: Option<&str>,
) -> Option<String> {
    match (local, endpoint) {
        (Some(local), Some(endpoint)) if local == endpoint => {
            Some(format!("client + endpoint: {local}"))
        }
        (Some(local), Some(endpoint)) => Some(format!("client: {local}\nendpoint: {endpoint}")),
        (Some(local), None) => Some(local.to_owned()),
        (None, Some(endpoint)) => Some(endpoint.to_owned()),
        (None, None) => None,
    }
}

impl ClientShellState {
    pub(super) fn set_local_config_diagnostic(&mut self, diagnostic: Option<String>) {
        self.local_config_diagnostic = diagnostic;
        self.config_diagnostic = merged_config_diagnostic(
            self.local_config_diagnostic.as_deref(),
            self.snapshot
                .as_deref()
                .and_then(|snapshot| snapshot.config_diagnostic.as_deref()),
        );
    }

    pub(crate) fn reload_client_config(&mut self) {
        match crate::config::load_live_config() {
            Ok(loaded) => {
                let diagnostics = self.config.apply_live_config(
                    &loaded.config,
                    &loaded.diagnostics,
                    &loaded.invalid_sections,
                );
                if let Some(appearance) = self.host_appearance {
                    self.config.palette = crate::app::client_palette_for_appearance(
                        &self.config.theme_runtime,
                        appearance,
                    );
                }
                self.set_local_config_diagnostic(self.config.local_config_diagnostic(&diagnostics));
            }
            Err(diagnostics) => {
                self.set_local_config_diagnostic(self.config.local_config_diagnostic(&diagnostics));
            }
        }
    }
}

impl ClientShellConfig {
    pub(crate) fn from_config(config: &Config) -> Self {
        Self {
            copy_on_select: config.ui.copy_on_select,
            clipboard_toast_enabled: config.ui.toast.clipboard.enabled,
            clipboard_toast_position: config.ui.toast.clipboard.position,
            theme_runtime: crate::app::client_theme_runtime_from_config(config),
            palette: crate::app::client_palette_from_config(config),
            mouse_capture: config.ui.mouse_capture,
            mouse_scroll_lines: config.ui.mouse_scroll_lines(),
            right_click_passthrough_modifiers: config.ui.right_click_passthrough_modifiers(),
            redraw_on_focus_gained: config.ui.redraw_on_focus_gained,
            startup_config_diagnostic: None,
        }
    }

    pub(crate) fn with_startup_config_diagnostic(mut self, diagnostic: Option<String>) -> Self {
        self.startup_config_diagnostic = diagnostic;
        self
    }

    pub(super) fn local_config_diagnostic(&self, diagnostics: &[String]) -> Option<String> {
        crate::config::config_diagnostic_summary(diagnostics)
    }

    pub(super) fn apply_live_config(
        &mut self,
        config: &Config,
        load_diagnostics: &[String],
        invalid_sections: &[String],
    ) -> Vec<String> {
        let mut diagnostics = load_diagnostics.to_vec();
        let invalid_section =
            |section: &str| invalid_sections.iter().any(|invalid| invalid == section);

        if !invalid_section("ui") {
            let ui = &config.ui;
            diagnostics.extend(ui.sound.diagnostics());
            self.copy_on_select = ui.copy_on_select;
            self.clipboard_toast_enabled = ui.toast.clipboard.enabled;
            self.clipboard_toast_position = ui.toast.clipboard.position;
            self.mouse_capture = ui.mouse_capture;
            self.mouse_scroll_lines = ui.mouse_scroll_lines();
            self.right_click_passthrough_modifiers = ui.right_click_passthrough_modifiers();
            self.redraw_on_focus_gained = ui.redraw_on_focus_gained;
        }

        if !invalid_section("theme") {
            self.theme_runtime = crate::app::client_theme_runtime_from_config(config);
            self.palette = crate::app::client_palette_from_config(config);
        }

        diagnostics
    }

    pub(crate) fn initial_surface_size(&self, cols: u16, rows: u16) -> ClientSurfaceSize {
        if crate::bus::entry::data_dir().is_some() {
            let surface = crate::client::rooms::layout(cols, rows).pane_surface;
            return ClientSurfaceSize {
                cols: surface.width.max(1),
                rows: surface.height.max(1),
            };
        }
        ClientSurfaceSize {
            cols: cols.max(1),
            rows: rows.max(1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_reload_applies_client_owned_sections() {
        let mut shell = ClientShellConfig::from_config(&Config::default());
        let mut next = Config::default();
        next.ui.copy_on_select = !shell.copy_on_select;
        next.ui.mouse_scroll_lines = std::num::NonZeroUsize::new(7);

        let diagnostics = shell.apply_live_config(&next, &[], &[]);

        assert!(diagnostics.is_empty());
        assert_eq!(shell.copy_on_select, next.ui.copy_on_select);
        assert_eq!(shell.mouse_scroll_lines, 7);
    }

    #[test]
    fn live_reload_preserves_invalid_client_owned_sections() {
        let mut initial = Config::default();
        initial.ui.mouse_scroll_lines = std::num::NonZeroUsize::new(4);
        let mut shell = ClientShellConfig::from_config(&initial);

        let mut invalid = Config::default();
        invalid.ui.mouse_scroll_lines = std::num::NonZeroUsize::new(9);
        shell.apply_live_config(&invalid, &[], &["ui".to_owned()]);

        assert_eq!(shell.mouse_scroll_lines, 4);
    }
}
