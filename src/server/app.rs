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
use super::workspaces::ids as terminal_targets;
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

use super::app_settings::{agent_panel_sort_from_config, parse_cjk_ime_agents};
use crate::utils::theme::theme_runtime_config;
pub(crate) use crate::utils::theme::{
    client_palette_for_appearance, client_palette_from_config, client_theme_runtime_from_config,
    resolve_effective_theme,
};
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

        let mut state =
            Self::initial_state(config, config_diagnostic, workspaces, active, selected);

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

    fn initial_state(
        config: &Config,
        config_diagnostic: Option<String>,
        workspaces: Vec<crate::server::workspaces::Workspace>,
        active: Option<usize>,
        selected: usize,
    ) -> AppState {
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

        AppState {
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
        }
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
}
#[cfg(test)]
#[path = "tests/app_support_test.rs"]
mod tests;
