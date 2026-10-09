use super::app::App;
use super::app_settings::{agent_panel_sort_from_config, parse_cjk_ime_agents};
use crate::utils::config;
use crate::utils::theme::theme_runtime_config;

impl App {
    pub(crate) fn reload_config(&mut self) -> config::ConfigReloadReport {
        self.apply_config_from_disk(true)
    }

    pub(crate) fn take_config_reloaded_from_disk(&mut self) -> bool {
        let reloaded = self.config_reloaded_from_disk;
        self.config_reloaded_from_disk = false;
        reloaded
    }

    pub(crate) fn apply_config_from_disk(
        &mut self,
        notify_success: bool,
    ) -> config::ConfigReloadReport {
        self.config_reloaded_from_disk = true;
        let previous_toast = self.state.toast.clone();
        let report = match config::load_live_config() {
            Ok(loaded) => self.apply_live_config(
                &loaded.config,
                &loaded.diagnostics,
                &loaded.invalid_sections,
                notify_success,
            ),
            Err(diagnostics) => {
                self.state.toast = None;
                self.state.config_diagnostic = config::config_diagnostic_summary(&diagnostics);
                self.config_diagnostic_deadline = None;
                config::ConfigReloadReport {
                    status: config::ConfigReloadStatus::Failed,
                    diagnostics,
                }
            }
        };
        self.sync_toast_deadline(previous_toast);
        report
    }

    fn apply_live_config(
        &mut self,
        config: &config::Config,
        load_diagnostics: &[String],
        invalid_sections: &[String],
        notify_success: bool,
    ) -> config::ConfigReloadReport {
        let mut diagnostics = load_diagnostics.to_vec();
        let invalid_section =
            |section: &str| invalid_sections.iter().any(|invalid| invalid == section);

        if !invalid_section("ui") {
            diagnostics.extend(config.ui.sound.diagnostics());
            diagnostics.extend(config::window_title_diagnostics(&config.ui.window_title));

            self.loaded_host_cursor = config.ui.host_cursor;
            self.state.confirm_close = config.ui.confirm_close;
            self.state.pane_borders = config.ui.pane_borders;
            self.state.pane_outer_borders = config.ui.pane_outer_borders;
            self.state.pane_scrollbars = config.ui.pane_scrollbars;
            self.state.pane_gaps = config.ui.pane_gaps;
            self.state.show_agent_labels_on_pane_borders =
                config.ui.show_agent_labels_on_pane_borders;
            self.configure_window_title(&config.ui.window_title);
            self.state.agent_panel_sort = agent_panel_sort_from_config(config.ui.agent_panel_sort);
            self.state.sound = config.ui.sound.clone();
            self.state.toast_config = config.ui.toast.clone();
        }

        let graphics_config_valid = !invalid_section("terminal")
            && (config.terminal.kitty_graphics.is_some() || !invalid_section("experimental"));
        if graphics_config_valid
            && config.kitty_graphics_enabled() != self.state.kitty_graphics_enabled
        {
            diagnostics.push(
                "terminal.kitty_graphics changes require restarting Bus; kept current setting"
                    .into(),
            );
        }

        if !invalid_section("experimental") {
            self.state.reveal_hidden_cursor_for_cjk_ime =
                config.experimental.reveal_hidden_cursor_for_cjk_ime;
            self.state.cjk_ime_agent_filter_configured =
                !config.experimental.cjk_ime_agents.is_empty();
            self.state.cjk_ime_agents = parse_cjk_ime_agents(&config.experimental.cjk_ime_agents);
            self.state.cjk_ime_cursor_shape =
                config.experimental.cjk_ime_cursor_shape.to_decscusr();
            self.persist_pane_history = config.experimental.pane_history;
            if !self.persist_pane_history {
                crate::server::persistence::clear_history();
            }
        }

        if !invalid_section("server") {
            if let Some(diagnostic) = config.invalid_headless_size_diagnostic() {
                diagnostics.push(format!("{diagnostic}; keeping current [server] settings"));
            } else {
                self.state.headless_size = config.headless_size();
            }
        }

        if !invalid_section("advanced") {
            self.state.pane_scrollback_limit_bytes = config.advanced.scrollback_limit_bytes;
        }

        if !invalid_section("terminal") {
            self.state.default_shell = config.terminal.default_shell.clone();
            self.state.shell_mode = config.terminal.shell_mode;
            self.state.new_terminal_cwd = config.terminal.new_cwd.clone();
        }

        if !invalid_section("theme") {
            self.state.theme_runtime = theme_runtime_config(config, !invalid_section("ui"));
            self.refresh_effective_app_theme();
        }

        let status = if diagnostics.is_empty() {
            config::ConfigReloadStatus::Applied
        } else {
            config::ConfigReloadStatus::Partial
        };

        if diagnostics.is_empty() {
            self.state.config_diagnostic = None;
            self.config_diagnostic_deadline = None;
            if notify_success {
                self.state.toast = Some(crate::server::app_state::ToastNotification {
                    kind: crate::server::app_state::ToastKind::UpdateInstalled,
                    title: "reloaded config".to_string(),
                    context: "using config.toml".to_string(),
                    position: None,
                    target: None,
                });
            }
        } else {
            self.state.config_diagnostic = config::config_diagnostic_summary(&diagnostics);
            self.config_diagnostic_deadline = None;
            if notify_success {
                self.state.toast = Some(crate::server::app_state::ToastNotification {
                    kind: crate::server::app_state::ToastKind::UpdateInstalled,
                    title: "reloaded config".to_string(),
                    context: "with warnings".to_string(),
                    position: None,
                    target: None,
                });
            }
        }

        self.state.request_client_config_reload = true;
        config::ConfigReloadReport {
            status,
            diagnostics,
        }
    }
}
