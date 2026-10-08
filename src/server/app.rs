//! Application orchestration.
//!
//! - `app_state.rs` — AppState, Mode, and pure data structs
//! - `terminals/events.rs` — state mutations (testable without PTYs/async)

// Keep the old app item paths while each implementation has one server-owned home.
pub(crate) use super::api::input_encoding as api_helpers;
#[cfg(test)]
pub(crate) use super::api::test_support::exiting_test_command;
pub(crate) use super::app_state as state;
pub(crate) use super::terminals::events as actions;
#[cfg(test)]
use super::workspaces::cwd as creation;
#[cfg(test)]
use super::workspaces::targets as terminal_targets;
pub(crate) use api_helpers::limit_snapshot_lines;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(super) const MIN_RENDER_INTERVAL: Duration = Duration::from_millis(16);
pub(super) const PENDING_AGENT_RESUME_THEME_WAIT: Duration = Duration::from_millis(750);
pub(super) const SESSION_SAVE_DEBOUNCE: Duration = Duration::from_secs(5);

use ratatui::layout::Rect;
use tokio::sync::{mpsc, Notify};
use tracing::info;

use crate::config::Config;
use crate::events::AppEvent;

pub use state::{AppState, Mode, ToastKind, ViewState};

/// Full application: AppState + runtime concerns (event channels, async I/O).
#[derive(Debug, Clone)]
pub(crate) struct OverlayPaneState {
    pub(super) ws_idx: usize,
    pub(super) tab_idx: usize,
    pub(super) previous_focus: crate::layout::PaneId,
    pub(super) previous_zoomed: bool,
    pub(super) temp_files: Vec<std::path::PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AppPolicy {
    pub(crate) restore_session: bool,
    pub(crate) persist_session: bool,
}

impl AppPolicy {
    pub(crate) const PRODUCTION: Self = Self {
        restore_session: true,
        persist_session: true,
    };

    #[cfg(test)]
    pub(crate) const TEST: Self = Self {
        restore_session: false,
        persist_session: false,
    };
}

pub struct App {
    pub state: AppState,
    pub(crate) pixel_mouse_available: bool,
    pub(crate) terminal_runtimes: crate::terminal::TerminalRuntimeRegistry,
    pub event_tx: mpsc::Sender<AppEvent>,
    pub(crate) event_rx: mpsc::Receiver<AppEvent>,
    pub(crate) api_rx: tokio::sync::mpsc::UnboundedReceiver<crate::api::ApiRequestMessage>,
    pub(crate) event_hub: crate::api::EventHub,
    pub(crate) last_focus: Option<(usize, crate::layout::PaneId)>,
    pub(crate) policy: AppPolicy,
    pub(crate) config_diagnostic_deadline: Option<Instant>,
    pub(crate) toast_deadline: Option<Instant>,
    pub(crate) last_api_notification_at: Option<Instant>,
    pub(crate) loaded_host_cursor: crate::config::HostCursorModeConfig,
    pub(crate) pending_agent_resume_deadline: Option<Instant>,
    pub(crate) session_save_deadline: Option<Instant>,
    pub(crate) session_save_thread: Option<std::thread::JoinHandle<()>>,
    pub(super) pane_exit_checkpoint_pending: bool,
    pub(crate) detached_process_children: Vec<std::process::Child>,
    /// Parsed `ui.window_title` plus the hostname resolved when it was applied.
    pub(super) window_title_template: Option<(crate::config::WindowTitleTemplate, String)>,
    pub(crate) persist_pane_history: bool,
    /// Last render-loop attempt, including a throttled hidden-only PTY skip.
    pub(crate) last_render_at: Option<Instant>,
    /// Last attempt that could update a connected presentation surface.
    pub(crate) last_presentation_at: Option<Instant>,
    pub render_notify: Arc<Notify>,
    pub(crate) render_dirty: Arc<crate::render_signal::RenderSignal>,
    pub(crate) full_redraw_pending: bool,
    pub(crate) overlay_panes: HashMap<crate::layout::PaneId, OverlayPaneState>,
    pub(crate) config_reloaded_from_disk: bool,
}

pub(crate) const APP_EVENT_CHANNEL_CAPACITY: usize = 256;
pub(crate) const APP_EVENT_DRAIN_LIMIT: usize = 64;

fn agent_panel_sort_from_config(
    sort: crate::config::AgentPanelSortConfig,
) -> state::AgentPanelSort {
    match sort {
        crate::config::AgentPanelSortConfig::Spaces => state::AgentPanelSort::Spaces,
        crate::config::AgentPanelSortConfig::Priority => state::AgentPanelSort::Priority,
    }
}

/// Parse the configured agent name list into a deduplicated set of `Agent`
/// values. Unknown agent names are silently dropped so a typo cannot disable
/// other valid entries.
fn parse_cjk_ime_agents(names: &[String]) -> Vec<crate::detect::Agent> {
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        if let Some(agent) = crate::detect::parse_agent_label(name) {
            if !out.contains(&agent) {
                out.push(agent);
            }
        }
    }
    out
}

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

fn theme_runtime_config(
    config: &crate::config::Config,
    use_legacy_ui_accent: bool,
) -> state::ThemeRuntimeConfig {
    let manual_name = config
        .theme
        .name
        .clone()
        .unwrap_or_else(|| "catppuccin".to_string());
    let (default_dark, default_light) = sibling_theme_names(&manual_name);
    state::ThemeRuntimeConfig {
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
    runtime: &state::ThemeRuntimeConfig,
    mode_custom: Option<&crate::config::ModeThemeColors>,
) -> state::Palette {
    let mut palette = state::Palette::from_name(name).unwrap_or_else(|| {
        tracing::warn!(
            theme = name,
            fallback = fallback_name,
            "unknown theme, falling back"
        );
        state::Palette::from_name(fallback_name).unwrap_or_else(state::Palette::catppuccin)
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

pub(super) fn resolve_effective_theme(
    runtime: &state::ThemeRuntimeConfig,
    appearance: Option<crate::terminal_theme::HostAppearance>,
) -> (state::Palette, String) {
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

pub(crate) fn client_theme_runtime_from_config(config: &Config) -> state::ThemeRuntimeConfig {
    theme_runtime_config(config, true)
}

pub(crate) fn client_palette_from_config(config: &Config) -> state::Palette {
    let runtime = client_theme_runtime_from_config(config);
    resolve_effective_theme(&runtime, None).0
}

pub(crate) fn client_palette_for_appearance(
    runtime: &state::ThemeRuntimeConfig,
    appearance: crate::terminal_theme::HostAppearance,
) -> state::Palette {
    resolve_effective_theme(runtime, Some(appearance)).0
}

impl App {
    pub fn new(
        config: &Config,
        policy: AppPolicy,
        config_diagnostic: Option<String>,
        api_rx: tokio::sync::mpsc::UnboundedReceiver<crate::api::ApiRequestMessage>,
        event_hub: crate::api::EventHub,
    ) -> Self {
        crate::kitty_graphics::set_enabled(config.kitty_graphics_enabled());
        let (event_tx, event_rx) = mpsc::channel::<AppEvent>(APP_EVENT_CHANNEL_CAPACITY);
        let render_notify = Arc::new(Notify::new());
        let render_dirty = Arc::new(crate::render_signal::RenderSignal::new());

        // Try to restore previous session
        let mut restored_terminals = std::collections::HashMap::new();
        let mut restored_terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let (workspaces, active, selected) = if !policy.restore_session {
            (Vec::new(), None, 0)
        } else if let Some(snap) = crate::persist::load() {
            let history = config
                .experimental
                .pane_history
                .then(crate::persist::load_history)
                .flatten();
            let (ws, terminals, terminal_runtimes) = crate::persist::restore(
                &snap,
                history.as_ref(),
                24,
                80,
                config.advanced.scrollback_limit_bytes,
                &config.terminal.default_shell,
                config.terminal.shell_mode,
                config.session.resume_agents_on_restore,
                event_tx.clone(),
                render_notify.clone(),
                render_dirty.clone(),
            );
            restored_terminals = terminals;
            restored_terminal_runtimes = terminal_runtimes.into();
            if ws.is_empty() {
                crate::logging::session_restored(0, "empty");
                (Vec::new(), None, 0)
            } else {
                crate::logging::session_restored(ws.len(), "ok");
                let active = snap.active.filter(|&i| i < ws.len());
                let selected = snap.selected.min(ws.len().saturating_sub(1));
                (ws, active, selected)
            }
        } else {
            (Vec::new(), None, 0)
        };

        let agent_panel_sort = agent_panel_sort_from_config(config.ui.agent_panel_sort);

        info!(
            pane_scrollback_limit_bytes = config.advanced.scrollback_limit_bytes,
            "using pane scrollback configuration"
        );

        let mode = if active.is_some() {
            state::Mode::Terminal
        } else {
            state::Mode::Navigate
        };

        let theme_runtime = theme_runtime_config(config, true);
        let (theme_palette, theme_name) = resolve_effective_theme(&theme_runtime, None);

        let mut state = AppState {
            terminals: std::collections::HashMap::new(),
            pane_id_aliases: std::collections::HashMap::new(),
            public_pane_id_aliases: std::collections::HashMap::new(),
            workspaces,
            active,
            previous_pane_focus: None,
            selected,
            mode,
            should_quit: false,
            request_client_config_reload: false,
            view: state::ViewState {
                terminal_area: Rect::default(),
                pane_infos: Vec::new(),
            },
            config_diagnostic,
            toast: None,
            pending_agent_notifications: std::collections::HashMap::new(),
            outer_terminal_focus: None,
            headless_size: config.headless_size(),
            agent_panel_sort,
            next_agent_state_change_seq: 0,
            confirm_close: config.ui.confirm_close,
            pane_borders: config.ui.pane_borders,
            pane_outer_borders: config.ui.pane_outer_borders,
            pane_scrollbars: config.ui.pane_scrollbars,
            pane_gaps: config.ui.pane_gaps,
            show_agent_labels_on_pane_borders: config.ui.show_agent_labels_on_pane_borders,
            reveal_hidden_cursor_for_cjk_ime: config.experimental.reveal_hidden_cursor_for_cjk_ime,
            cjk_ime_agent_filter_configured: !config.experimental.cjk_ime_agents.is_empty(),
            cjk_ime_agents: parse_cjk_ime_agents(&config.experimental.cjk_ime_agents),
            cjk_ime_cursor_shape: config.experimental.cjk_ime_cursor_shape.to_decscusr(),
            kitty_graphics_enabled: config.kitty_graphics_enabled(),
            default_shell: config.terminal.default_shell.clone(),
            shell_mode: config.terminal.shell_mode,
            new_terminal_cwd: config.terminal.new_cwd.clone(),
            pane_scrollback_limit_bytes: config.advanced.scrollback_limit_bytes,
            sound: config.ui.sound.clone(),
            toast_config: config.ui.toast.clone(),
            palette: theme_palette,
            theme_name,
            theme_runtime,
            host_terminal_appearance: None,
            host_terminal_appearance_explicit: false,
            host_terminal_theme: crate::terminal_theme::TerminalTheme::default(),
            host_cell_size: crate::kitty_graphics::HostCellSize::default(),
            session_dirty: false,
            terminal_runtime_shutdowns: Vec::new(),
        };

        state.terminals = restored_terminals;

        state.refresh_workspace_auto_labels(&restored_terminal_runtimes);

        let last_focus = state.active.and_then(|idx| {
            state
                .workspaces
                .get(idx)
                .and_then(|ws| ws.focused_pane_id().map(|pane_id| (idx, pane_id)))
        });

        let mut app = Self {
            config_diagnostic_deadline: None,
            toast_deadline: None,
            last_api_notification_at: None,
            state,
            pixel_mouse_available: false,
            terminal_runtimes: restored_terminal_runtimes,
            event_tx,
            event_rx,
            loaded_host_cursor: config.ui.host_cursor,
            pending_agent_resume_deadline: None,
            session_save_deadline: None,
            session_save_thread: None,
            pane_exit_checkpoint_pending: false,
            detached_process_children: Vec::new(),
            window_title_template: None,
            persist_pane_history: config.experimental.pane_history,
            last_render_at: None,
            last_presentation_at: None,
            api_rx,
            event_hub,
            last_focus,
            policy,
            render_notify,
            render_dirty,
            full_redraw_pending: false,
            overlay_panes: HashMap::new(),
            config_reloaded_from_disk: false,
        };
        app.configure_window_title(&config.ui.window_title);
        app
    }

    pub(crate) fn ensure_default_workspace(&mut self) -> bool {
        if !self.state.workspaces.is_empty() {
            return false;
        }

        let cwd = self.resolve_new_terminal_cwd(None);
        let preserve_checkpoint = self.pane_exit_checkpoint_pending && !self.state.session_dirty;

        match self.create_workspace_with_options(cwd, true) {
            Ok(_) => {
                if preserve_checkpoint {
                    // Automatic replacement is part of pane removal, not a new user mutation.
                    self.pane_exit_checkpoint_pending = true;
                    self.finish_checkpointed_pane_exit();
                }
                true
            }
            Err(err) => {
                tracing::error!(err = %err, "failed to create default workspace");
                self.state.mode = Mode::Navigate;
                false
            }
        }
    }

    pub(crate) fn reload_config(&mut self) -> crate::config::ConfigReloadReport {
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
    ) -> crate::config::ConfigReloadReport {
        self.config_reloaded_from_disk = true;
        let previous_toast = self.state.toast.clone();
        let report = match crate::config::load_live_config() {
            Ok(loaded) => self.apply_live_config(
                &loaded.config,
                &loaded.diagnostics,
                &loaded.invalid_sections,
                notify_success,
            ),
            Err(diagnostics) => {
                self.state.toast = None;
                self.state.config_diagnostic =
                    crate::config::config_diagnostic_summary(&diagnostics);
                self.config_diagnostic_deadline = None;
                crate::config::ConfigReloadReport {
                    status: crate::config::ConfigReloadStatus::Failed,
                    diagnostics,
                }
            }
        };
        self.sync_toast_deadline(previous_toast);
        report
    }

    fn apply_live_config(
        &mut self,
        config: &crate::config::Config,
        load_diagnostics: &[String],
        invalid_sections: &[String],
        notify_success: bool,
    ) -> crate::config::ConfigReloadReport {
        let mut diagnostics = load_diagnostics.to_vec();
        let invalid_section =
            |section: &str| invalid_sections.iter().any(|invalid| invalid == section);

        if !invalid_section("ui") {
            diagnostics.extend(config.ui.sound.diagnostics());
            diagnostics.extend(crate::config::window_title_diagnostics(
                &config.ui.window_title,
            ));

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
                "terminal.kitty_graphics changes require restarting Herdr; kept current setting"
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
                crate::persist::clear_history();
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
            crate::config::ConfigReloadStatus::Applied
        } else {
            crate::config::ConfigReloadStatus::Partial
        };

        if diagnostics.is_empty() {
            self.state.config_diagnostic = None;
            self.config_diagnostic_deadline = None;
            if notify_success {
                self.state.toast = Some(crate::app::state::ToastNotification {
                    kind: crate::app::state::ToastKind::UpdateInstalled,
                    title: "reloaded config".to_string(),
                    context: "using config.toml".to_string(),
                    position: None,
                    target: None,
                });
            }
        } else {
            self.state.config_diagnostic = crate::config::config_diagnostic_summary(&diagnostics);
            self.config_diagnostic_deadline = None;
            if notify_success {
                self.state.toast = Some(crate::app::state::ToastNotification {
                    kind: crate::app::state::ToastKind::UpdateInstalled,
                    title: "reloaded config".to_string(),
                    context: "with warnings".to_string(),
                    position: None,
                    target: None,
                });
            }
        }

        self.state.request_client_config_reload = true;
        crate::config::ConfigReloadReport {
            status,
            diagnostics,
        }
    }
}
#[cfg(test)]
#[path = "tests/app_support_test.rs"]
mod tests;
