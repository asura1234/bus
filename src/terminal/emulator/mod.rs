use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use tokio::sync::mpsc;

use crate::protocol::CellData;

mod color_replies;
#[cfg(windows)]
mod conpty_recent_cache;
mod controls;
mod dirty_patch;
mod encode;
mod read;
mod render;
mod text_motion;
#[cfg(windows)]
mod windows;
mod write;

use self::controls::cursor::{CursorPositionSettleState, DecscusrTracker};
use self::controls::kitty_keyboard::KittyKeyboardTracker;
use self::controls::osc::{
    AgentOscStateTracker, DefaultColorEventTracker, DefaultColorOscTracker, OscDebugTracker,
};
use self::controls::xtgettcap::XtgettcapQueryTracker;
pub(super) use self::render::ghostty_blank_symbol_for_width;
use self::render::{current_cursor_state, ghostty_line_from_cells};

#[cfg(test)]
use self::color_replies::should_probe_host_terminal_theme_restore;
#[cfg(test)]
use self::controls::cursor::CURSOR_POSITION_SETTLE;
#[cfg(all(test, windows))]
use self::read::finish_recent_snapshot;
#[cfg(test)]
use self::render::{
    decscusr_cursor_shape, ghostty_buffer_symbol_into, ghostty_cell_color,
    ghostty_symbol_fits_cell, PaletteOverrides,
};
#[cfg(test)]
use self::text_motion::RetainedTextBuffer;
#[cfg(all(test, windows))]
use self::windows::windows_powershell_prompt_cwd;
#[cfg(test)]
use self::write::render_delay_after_pty_write;
#[cfg(test)]
use crate::layout::PaneId;
#[cfg(test)]
use ratatui::style::Modifier;

const KITTY_GRAPHICS_REDRAW_SETTLE: Duration = Duration::from_millis(20);
const CURSOR_POSITION_SETTLE_ENABLED: bool = cfg!(windows);
const MODE_MOUSE_X10: u16 = 9;
const MODE_MOUSE_PRESS_RELEASE: u16 = 1000;
const MODE_MOUSE_BUTTON_MOTION: u16 = 1002;
const MODE_MOUSE_ANY_MOTION: u16 = 1003;

pub use crate::utils::render::widgets::ScrollMetrics;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TerminalTextPoint {
    pub row: u32,
    pub col: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TerminalTextMatch {
    pub start: TerminalTextPoint,
    pub end: TerminalTextPoint,
    pub source_fingerprint: u64,
    pub scan_cols: u16,
    pub scan_screen: crate::ghostty::ActiveScreen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalSearchDirection {
    Forward,
    Backward,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TerminalSearchWindow {
    pub matches: Vec<TerminalTextMatch>,
    pub current: Option<usize>,
    pub current_global: Option<usize>,
    pub total: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalWordMotion {
    NextStart,
    PreviousStart,
    NextEnd,
    NextBigStart,
    PreviousBigStart,
    NextBigEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalCursorState {
    pub x: u16,
    pub y: u16,
    pub visible: bool,
    /// DECSCUSR parameter (0–6). 0 means terminal default.
    pub shape: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TerminalDirtyPatch {
    pub rows: Vec<(u16, Vec<CellData>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TerminalDirtyPatchOutcome {
    Clean,
    Patch(TerminalDirtyPatch),
    Fallback,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcessBytesResult {
    pub request_render: bool,
    pub render_delay: Option<Duration>,
    pub terminal_title_changed: bool,
    pub terminal_bells: u16,
    pub clipboard_writes: Vec<Vec<u8>>,
    pub reported_cwd: Option<std::path::PathBuf>,
    pub terminal_responses: Vec<Bytes>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct TerminalReadSnapshot {
    pub text: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalCompressionStep {
    Busy,
    ActivityChanged(u64),
    Compressed(crate::ghostty::TerminalCompressionResult),
}

pub(crate) struct GhosttyPaneTerminal {
    pub core: Mutex<GhosttyPaneCore>,
    key_encoder: Mutex<crate::ghostty::KeyEncoder>,
    pending_pty_responses: Arc<Mutex<Vec<Bytes>>>,
}

pub(crate) struct GhosttyPaneCore {
    pub terminal: crate::ghostty::Terminal,
    #[cfg(windows)]
    recent_fallback: conpty_recent_cache::Cache,
    pub render_state: crate::ghostty::RenderState,
    pub kitty_keyboard: KittyKeyboardTracker,
    pub initial_default_foreground: Option<crate::ghostty::RgbColor>,
    pub initial_default_background: Option<crate::ghostty::RgbColor>,
    pub host_terminal_theme: crate::terminal_theme::TerminalTheme,
    pub transient_default_color_owner_pgid: Option<u32>,
    // Control tracker types stay internal to the emulator, as do these fields.
    default_color_tracker: DefaultColorOscTracker,
    default_color_event_tracker: DefaultColorEventTracker,
    pub child_default_foreground_changed: bool,
    pub child_default_background_changed: bool,
    osc_debug_tracker: OscDebugTracker,
    agent_osc_state: AgentOscStateTracker,
    xtgettcap_query_tracker: XtgettcapQueryTracker,
    decscusr_tracker: DecscusrTracker,
    cursor_settle_state: CursorPositionSettleState,
    windows_powershell_prompt_cwd_reporting: bool,
}

pub(crate) struct PaneTerminal {
    pub(crate) ghostty: GhosttyPaneTerminal,
}

impl PaneTerminal {
    pub(crate) fn new(ghostty: GhosttyPaneTerminal) -> Self {
        Self { ghostty }
    }

    pub fn scroll_up(&self, lines: usize) {
        self.ghostty.scroll_up(lines);
    }

    pub fn scroll_down(&self, lines: usize) {
        self.ghostty.scroll_down(lines);
    }

    pub fn scroll_reset(&self) {
        self.ghostty.scroll_reset();
    }

    pub fn set_scroll_offset_from_bottom(&self, lines: usize) {
        self.ghostty.set_scroll_offset_from_bottom(lines);
    }

    pub fn scroll_metrics(&self) -> Option<ScrollMetrics> {
        self.ghostty.scroll_metrics()
    }

    pub fn bracketed_paste_enabled(&self) -> bool {
        self.ghostty.bracketed_paste_enabled()
    }

    pub fn focus_reporting_enabled(&self) -> bool {
        self.ghostty.focus_reporting_enabled()
    }

    pub fn mouse_reporting_enabled(&self) -> bool {
        self.ghostty.mouse_reporting_enabled()
    }

    pub fn modify_other_keys_level(&self) -> u8 {
        self.ghostty.modify_other_keys_level()
    }

    pub fn sgr_pixel_mouse_enabled(&self) -> bool {
        self.ghostty.sgr_pixel_mouse_enabled()
    }

    pub fn plain_page_keys_use_host_scrollback(&self) -> Option<bool> {
        self.ghostty.plain_page_keys_use_host_scrollback()
    }

    pub fn alternate_screen_active(&self) -> bool {
        self.ghostty.alternate_screen_active()
    }

    pub fn wheel_routing(&self) -> Option<crate::terminal::runtime::WheelRouting> {
        self.ghostty.wheel_routing()
    }

    pub(crate) fn screen_text_snapshot(
        &self,
    ) -> Option<(
        crate::ghostty::ActiveScreen,
        u16,
        Vec<crate::ghostty::ScreenTextRow>,
    )> {
        self.ghostty.screen_text_snapshot()
    }

    pub fn cursor_state(&self) -> Option<TerminalCursorState> {
        self.ghostty.cursor_state()
    }

    pub fn synchronized_output_active(&self) -> bool {
        self.ghostty.synchronized_output_active()
    }

    pub fn visible_text(&self) -> String {
        self.ghostty.visible_text()
    }

    pub fn visible_ansi(&self) -> String {
        self.ghostty.visible_ansi()
    }

    pub fn detection_text(&self) -> String {
        self.ghostty.detection_text()
    }

    pub(crate) fn try_compression_activity(&self) -> Result<Option<u64>, crate::ghostty::Error> {
        self.ghostty.try_compression_activity()
    }

    pub(crate) fn try_compress_incremental_if_activity(
        &self,
        expected_activity: u64,
    ) -> Result<TerminalCompressionStep, crate::ghostty::Error> {
        self.ghostty
            .try_compress_incremental_if_activity(expected_activity)
    }

    pub(crate) fn recent_text_snapshot(&self, lines: usize) -> TerminalReadSnapshot {
        self.ghostty.recent_text_snapshot(lines)
    }

    pub(crate) fn recent_ansi_snapshot(&self, lines: usize) -> TerminalReadSnapshot {
        self.ghostty.recent_ansi_snapshot(lines)
    }

    pub(crate) fn recent_unwrapped_text_snapshot(&self, lines: usize) -> TerminalReadSnapshot {
        self.ghostty.recent_unwrapped_text_snapshot(lines)
    }

    pub fn recent_unwrapped_ansi(&self, lines: usize) -> String {
        self.ghostty.recent_unwrapped_ansi(lines)
    }

    pub(crate) fn recent_unwrapped_ansi_snapshot(&self, lines: usize) -> TerminalReadSnapshot {
        self.ghostty.recent_unwrapped_ansi_snapshot(lines)
    }

    pub fn extract_selection(&self, selection: &crate::selection::Selection) -> Option<String> {
        self.ghostty.extract_selection(selection)
    }

    pub fn terminal_title(&self) -> Option<String> {
        self.ghostty.terminal_title()
    }

    pub fn agent_osc_title(&self) -> String {
        self.ghostty.agent_osc_title()
    }

    pub fn agent_osc_progress(&self) -> String {
        self.ghostty.agent_osc_progress()
    }

    /// Clears retained OSC title/progress evidence on foreground agent change.
    pub fn clear_agent_osc_state(&self) {
        self.ghostty.clear_agent_osc_state()
    }

    pub fn keyboard_protocol(
        &self,
        fallback: crate::input::KeyboardProtocol,
    ) -> crate::input::KeyboardProtocol {
        self.ghostty.keyboard_protocol().unwrap_or(fallback)
    }
}

impl GhosttyPaneTerminal {
    pub fn new(
        mut terminal: crate::ghostty::Terminal,
        _response_writer: mpsc::Sender<Bytes>,
    ) -> std::io::Result<Self> {
        let pending_pty_responses = Arc::new(Mutex::new(Vec::new()));
        let callback_responses = pending_pty_responses.clone();
        terminal
            .set_write_pty_callback(move |bytes| {
                if let Ok(mut responses) = callback_responses.lock() {
                    responses.push(Bytes::copy_from_slice(bytes));
                }
            })
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        let mut render_state =
            crate::ghostty::RenderState::new().map_err(|e| std::io::Error::other(e.to_string()))?;
        let initial_colors = render_state
            .update(&terminal)
            .ok()
            .and_then(|_| render_state.colors().ok());
        let initial_default_foreground = initial_colors.map(|colors| colors.foreground);
        let initial_default_background = initial_colors.map(|colors| colors.background);
        let mut key_encoder =
            crate::ghostty::KeyEncoder::new().map_err(|e| std::io::Error::other(e.to_string()))?;
        key_encoder.set_from_terminal(&terminal);
        Ok(Self {
            core: Mutex::new(GhosttyPaneCore {
                terminal,
                #[cfg(windows)]
                recent_fallback: conpty_recent_cache::Cache::default(),
                render_state,
                kitty_keyboard: KittyKeyboardTracker::default(),
                initial_default_foreground,
                initial_default_background,
                host_terminal_theme: crate::terminal_theme::TerminalTheme::default(),
                transient_default_color_owner_pgid: None,
                default_color_tracker: DefaultColorOscTracker::default(),
                default_color_event_tracker: DefaultColorEventTracker::default(),
                child_default_foreground_changed: false,
                child_default_background_changed: false,
                osc_debug_tracker: OscDebugTracker::default(),
                agent_osc_state: AgentOscStateTracker::default(),
                xtgettcap_query_tracker: XtgettcapQueryTracker::default(),
                decscusr_tracker: DecscusrTracker::default(),
                cursor_settle_state: CursorPositionSettleState::default(),
                windows_powershell_prompt_cwd_reporting: false,
            }),
            key_encoder: Mutex::new(key_encoder),
            pending_pty_responses,
        })
    }

    pub(super) fn set_windows_powershell_prompt_cwd_reporting(&self, enabled: bool) {
        if let Ok(mut core) = self.core.lock() {
            core.windows_powershell_prompt_cwd_reporting = enabled;
        }
    }

    pub fn terminal_title(&self) -> Option<String> {
        self.core
            .lock()
            .ok()
            .and_then(|core| core.agent_osc_state.terminal_title().map(str::to_string))
    }

    /// Returns the latest OSC 0/2 title retained for agent detection, or `""`
    /// if no title has been seen or the last update was an empty clear.
    pub fn agent_osc_title(&self) -> String {
        self.core
            .lock()
            .map(|core| core.agent_osc_state.latest_title().to_owned())
            .unwrap_or_default()
    }

    /// Returns the latest OSC 9 progress payload retained for agent detection,
    /// or `""` if none has been seen.
    pub fn agent_osc_progress(&self) -> String {
        self.core
            .lock()
            .map(|core| core.agent_osc_state.latest_progress().to_owned())
            .unwrap_or_default()
    }

    /// Clears retained OSC title/progress evidence when the pane's foreground
    /// agent changes, so a new agent process starts from a blank OSC slate.
    pub fn clear_agent_osc_state(&self) {
        if let Ok(mut core) = self.core.lock() {
            core.agent_osc_state.clear_retained();
        }
    }

    pub fn scroll_up(&self, lines: usize) {
        if let Ok(mut core) = self.core.lock() {
            #[cfg(windows)]
            conpty_recent_cache::refresh_if_needed(&mut core);
            core.terminal.scroll_viewport_delta(-(lines as isize));
        }
    }

    pub fn scroll_down(&self, lines: usize) {
        if let Ok(mut core) = self.core.lock() {
            core.terminal.scroll_viewport_delta(lines as isize);
        }
    }

    pub fn scroll_reset(&self) {
        if let Ok(mut core) = self.core.lock() {
            core.terminal.scroll_viewport_bottom();
        }
    }

    pub fn set_scroll_offset_from_bottom(&self, lines: usize) {
        if let Ok(mut core) = self.core.lock() {
            #[cfg(windows)]
            conpty_recent_cache::refresh_if_needed(&mut core);
            ghostty_set_scroll_offset_from_bottom(&mut core.terminal, lines);
        }
    }

    pub fn scroll_metrics(&self) -> Option<ScrollMetrics> {
        let Ok(core) = self.core.lock() else {
            return None;
        };
        let scrollbar = core.terminal.scrollbar().ok()?;
        Some(ScrollMetrics {
            offset_from_bottom: scrollbar
                .total
                .saturating_sub(scrollbar.offset + scrollbar.len),
            max_offset_from_bottom: scrollbar.total.saturating_sub(scrollbar.len),
            viewport_rows: scrollbar.len,
        })
    }

    pub fn keyboard_protocol(&self) -> Option<crate::input::KeyboardProtocol> {
        let Ok(core) = self.core.lock() else {
            return None;
        };
        Some(crate::input::KeyboardProtocol::from_kitty_flags(
            core.terminal.kitty_keyboard_flags().ok()? as u16,
        ))
    }

    pub fn bracketed_paste_enabled(&self) -> bool {
        self.mode_enabled(crate::ghostty::MODE_BRACKETED_PASTE)
    }

    pub fn focus_reporting_enabled(&self) -> bool {
        self.mode_enabled(crate::ghostty::MODE_FOCUS_EVENT)
    }

    pub fn mouse_reporting_enabled(&self) -> bool {
        self.core
            .lock()
            .is_ok_and(|core| core.terminal.mouse_tracking_enabled().unwrap_or(false))
    }

    pub fn modify_other_keys_level(&self) -> u8 {
        self.core
            .lock()
            .map_or(0, |core| core.kitty_keyboard.modify_other_keys_level())
    }

    pub fn sgr_pixel_mouse_enabled(&self) -> bool {
        self.mode_enabled(crate::ghostty::MODE_MOUSE_SGR_PIXELS)
    }

    fn mode_enabled(&self, mode: u16) -> bool {
        self.core
            .lock()
            .is_ok_and(|core| core.terminal.mode_get(mode).unwrap_or(false))
    }

    pub fn plain_page_keys_use_host_scrollback(&self) -> Option<bool> {
        let core = self.core.lock().ok()?;
        let alternate_screen =
            core.terminal.active_screen().ok()? == crate::ghostty::ActiveScreen::Alternate;
        let mouse_reporting = core.terminal.mouse_tracking_enabled().ok()?;
        let application_cursor = core
            .terminal
            .mode_get(crate::ghostty::MODE_APPLICATION_CURSOR_KEYS)
            .ok()?;
        let bracketed_paste = core
            .terminal
            .mode_get(crate::ghostty::MODE_BRACKETED_PASTE)
            .ok()?;
        Some(!alternate_screen && !mouse_reporting && (!application_cursor || bracketed_paste))
    }

    pub fn alternate_screen_active(&self) -> bool {
        self.core.lock().is_ok_and(|core| {
            core.terminal.active_screen().ok() == Some(crate::ghostty::ActiveScreen::Alternate)
        })
    }

    pub fn wheel_routing(&self) -> Option<crate::terminal::runtime::WheelRouting> {
        let Ok(core) = self.core.lock() else {
            return None;
        };
        let alternate_screen =
            core.terminal.active_screen().ok()? == crate::ghostty::ActiveScreen::Alternate;
        let mouse_alternate_scroll = core
            .terminal
            .mode_get(crate::ghostty::MODE_MOUSE_ALTERNATE_SCROLL)
            .ok()?;
        let mouse_reporting = core.terminal.mode_get(MODE_MOUSE_ANY_MOTION).ok()?
            || core.terminal.mode_get(MODE_MOUSE_BUTTON_MOTION).ok()?
            || core.terminal.mode_get(MODE_MOUSE_PRESS_RELEASE).ok()?
            || core.terminal.mode_get(MODE_MOUSE_X10).ok()?;
        Some(if mouse_reporting {
            crate::terminal::runtime::WheelRouting::MouseReport
        } else if alternate_screen && mouse_alternate_scroll {
            crate::terminal::runtime::WheelRouting::AlternateScroll
        } else {
            crate::terminal::runtime::WheelRouting::HostScroll
        })
    }

    pub fn cursor_state(&self) -> Option<TerminalCursorState> {
        let mut core = self.core.lock().ok()?;
        let current = current_cursor_state(&mut core);
        effective_cursor_state(&mut core, current)
    }

    pub fn synchronized_output_active(&self) -> bool {
        self.core
            .lock()
            .ok()
            .and_then(|core| {
                core.terminal
                    .mode_get(crate::ghostty::MODE_SYNCHRONIZED_OUTPUT)
                    .ok()
            })
            .unwrap_or(false)
    }

    fn try_lock_core(&self) -> Option<std::sync::MutexGuard<'_, GhosttyPaneCore>> {
        match self.core.try_lock() {
            Ok(core) => Some(core),
            Err(std::sync::TryLockError::WouldBlock) => None,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
        }
    }

    pub(crate) fn try_compression_activity(&self) -> Result<Option<u64>, crate::ghostty::Error> {
        let Some(core) = self.try_lock_core() else {
            return Ok(None);
        };
        core.terminal.compression_activity().map(Some)
    }

    pub(crate) fn try_compress_incremental_if_activity(
        &self,
        expected_activity: u64,
    ) -> Result<TerminalCompressionStep, crate::ghostty::Error> {
        let Some(mut core) = self.try_lock_core() else {
            return Ok(TerminalCompressionStep::Busy);
        };
        let activity = core.terminal.compression_activity()?;
        if activity != expected_activity {
            return Ok(TerminalCompressionStep::ActivityChanged(activity));
        }
        core.terminal
            .compress_incremental()
            .map(TerminalCompressionStep::Compressed)
    }
}

fn cursor_position_settle_pending(core: &GhosttyPaneCore) -> bool {
    core.cursor_settle_state.pending()
}

fn effective_cursor_state(
    core: &mut GhosttyPaneCore,
    current: Option<TerminalCursorState>,
) -> Option<TerminalCursorState> {
    if !CURSOR_POSITION_SETTLE_ENABLED {
        return current;
    }
    core.cursor_settle_state
        .reported_cursor(current, Instant::now())
}

fn ghostty_set_scroll_offset_from_bottom(
    terminal: &mut crate::ghostty::Terminal,
    offset_from_bottom: usize,
) {
    let Ok(scrollbar) = terminal.scrollbar() else {
        terminal.scroll_viewport_bottom();
        return;
    };
    let max_offset = scrollbar.total.saturating_sub(scrollbar.len);
    let offset_from_bottom = offset_from_bottom.min(max_offset);
    if offset_from_bottom == 0 {
        terminal.scroll_viewport_bottom();
    } else {
        terminal.scroll_viewport_row(max_offset - offset_from_bottom);
    }
}

pub(super) fn trim_trailing_blank_rows(rows: &mut Vec<String>) {
    while rows.last().is_some_and(|row| row.trim().is_empty()) {
        rows.pop();
    }
}

#[cfg(test)]
#[path = "tests/render_test.rs"]
mod tests;
