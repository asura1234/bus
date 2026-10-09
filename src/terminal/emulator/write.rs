use std::borrow::Cow;
use std::time::Instant;

use bytes::Bytes;
use tokio::sync::mpsc;
use tracing::{debug, error};

use super::color_replies::{
    default_color_event_color, remove_last_matching_libghostty_color_reply,
    respond_to_default_color_event,
};
use super::controls::cursor::CURSOR_POSITION_SETTLE;
use super::controls::osc::{
    contains_scrollback_clear_sequence, current_transient_default_color_owner,
    maybe_filter_primary_screen_scrollback_clear, parse_reported_cwd, DefaultColorEvent,
    DefaultColorTrackedEvent,
};
use super::controls::xtgettcap::XtgettcapResponse;
use super::ghostty_set_scroll_offset_from_bottom;
use super::read::{
    ghostty_detection_text, ghostty_recent_ansi, ghostty_visible_text, DEFAULT_DETECTION_ROWS,
};
#[cfg(windows)]
use super::{conpty_recent_cache, windows::windows_powershell_current_prompt_cwd};
use super::{
    current_cursor_state, cursor_position_settle_pending, GhosttyPaneCore, GhosttyPaneTerminal,
    PaneTerminal, ProcessBytesResult, CURSOR_POSITION_SETTLE_ENABLED, KITTY_GRAPHICS_REDRAW_SETTLE,
};
use crate::utils::ids::PaneId;
use std::time::Duration;

impl PaneTerminal {
    pub fn process_pty_bytes(
        &self,
        pane_id: PaneId,
        shell_pid: u32,
        bytes: &[u8],
        response_writer: &mpsc::Sender<Bytes>,
        foreground_job: impl Fn(u32) -> Option<crate::platform::ForegroundJob>,
    ) -> ProcessBytesResult {
        self.ghostty
            .process_pty_bytes(pane_id, shell_pid, bytes, response_writer, foreground_job)
    }

    pub fn resize(
        &self,
        rows: u16,
        cols: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Vec<Bytes> {
        self.ghostty
            .resize(rows, cols, cell_width_px, cell_height_px)
    }
}

impl GhosttyPaneTerminal {
    pub fn process_pty_bytes(
        &self,
        pane_id: PaneId,
        shell_pid: u32,
        bytes: &[u8],
        _response_writer: &mpsc::Sender<Bytes>,
        foreground_job: impl Fn(u32) -> Option<crate::platform::ForegroundJob>,
    ) -> ProcessBytesResult {
        crate::utils::render::prof::counter("pty.bytes", bytes.len() as u64);
        let Ok(mut core) = self.core.lock() else {
            error!(pane = pane_id.raw(), "ghostty core lock poisoned in reader");
            return ProcessBytesResult {
                request_render: false,
                render_delay: None,
                terminal_title_changed: false,
                terminal_bells: 0,
                clipboard_writes: Vec::new(),
                reported_cwd: None,
                terminal_responses: Vec::new(),
            };
        };

        let (terminal_title_changed, filtered_bytes) =
            observe_pty_bytes(&mut core, pane_id, shell_pid, bytes, foreground_job);

        core.kitty_keyboard.observe(filtered_bytes.as_ref());
        let mut terminal_responses = Vec::new();
        core.default_color_event_tracker
            .observe(filtered_bytes.as_ref());
        core.xtgettcap_query_tracker
            .observe(filtered_bytes.as_ref());
        core.decscusr_tracker.observe(filtered_bytes.as_ref());
        let in_progress_default_color_event = core.default_color_event_tracker.in_progress_event();
        let default_color_events = core.default_color_event_tracker.drain_pending();
        let xtgettcap_responses = core.xtgettcap_query_tracker.drain_pending();
        let write_started = crate::utils::render::prof::timer();
        self.write_pty_bytes_with_ordered_responses(
            &mut core,
            filtered_bytes.as_ref(),
            default_color_events,
            in_progress_default_color_event,
            xtgettcap_responses,
            &mut terminal_responses,
        );
        let terminal_bells = core.terminal.take_bell_count();
        let clipboard_writes = core.terminal.take_clipboard_writes();
        let reported_cwd = core
            .terminal
            .take_pwd_changes()
            .into_iter()
            .filter_map(|value| parse_reported_cwd(&value))
            .next_back();
        #[cfg(windows)]
        conpty_recent_cache::update_after_write(&mut core);
        crate::utils::render::prof::duration_since("pty.ghostty_write", write_started);

        let has_kitty_graphics_sequence = crate::protocol::kitty::is_enabled()
            && contains_kitty_graphics_sequence(filtered_bytes.as_ref());
        if has_kitty_graphics_sequence {
            debug!(pane = pane_id.raw(), "processed kitty graphics sequence");
        }
        if let Ok(mut key_encoder) = self.key_encoder.lock() {
            key_encoder.set_from_terminal(&core.terminal);
        }
        let synchronized_output = core
            .terminal
            .mode_get(crate::terminal::vt::MODE_SYNCHRONIZED_OUTPUT)
            .unwrap_or(false);
        if CURSOR_POSITION_SETTLE_ENABLED {
            let cursor_started = crate::utils::render::prof::timer();
            let cursor_after_write = current_cursor_state(&mut core);
            crate::utils::render::prof::duration_since("pty.cursor_state_update", cursor_started);
            core.cursor_settle_state
                .observe(cursor_after_write, Instant::now());
        }
        #[cfg(windows)]
        let reported_cwd = if core.windows_powershell_prompt_cwd_reporting {
            reported_cwd.or_else(|| windows_powershell_current_prompt_cwd(&mut core))
        } else {
            reported_cwd
        };

        let request_render = !synchronized_output;
        let render_delay = render_delay_after_pty_write(
            synchronized_output,
            has_kitty_graphics_sequence,
            cursor_position_settle_pending(&core),
            CURSOR_POSITION_SETTLE_ENABLED,
        );
        if request_render {
            crate::utils::render::prof::event("pty.request_render");
        }
        if render_delay.is_some() {
            crate::utils::render::prof::event("pty.request_render_delayed");
        }
        if synchronized_output {
            crate::utils::render::prof::event("pty.synchronized_output_suppressed");
        }
        ProcessBytesResult {
            request_render,
            render_delay,
            terminal_title_changed,
            terminal_bells,
            clipboard_writes,
            reported_cwd,
            terminal_responses,
        }
    }

    fn write_pty_bytes_with_ordered_responses(
        &self,
        core: &mut GhosttyPaneCore,
        bytes: &[u8],
        default_color_events: Vec<DefaultColorTrackedEvent>,
        in_progress_default_color_event: Option<DefaultColorEvent>,
        xtgettcap_responses: Vec<XtgettcapResponse>,
        terminal_responses: &mut Vec<Bytes>,
    ) {
        let mut events = Vec::with_capacity(default_color_events.len() + xtgettcap_responses.len());
        events.extend(
            default_color_events
                .into_iter()
                .map(OrderedPtyResponseEvent::DefaultColor),
        );
        events.extend(
            xtgettcap_responses
                .into_iter()
                .map(OrderedPtyResponseEvent::Xtgettcap),
        );
        events.sort_by_key(OrderedPtyResponseEvent::end_offset);

        let mut written = 0;
        for event in events {
            let end_offset = event.end_offset().min(bytes.len());
            let mut libghostty_responses = Vec::new();
            if end_offset > written {
                core.terminal.write(&bytes[written..end_offset]);
                libghostty_responses = self.drain_pending_pty_responses();
                written = end_offset;
            }
            match event {
                OrderedPtyResponseEvent::DefaultColor(event) => {
                    let replacement =
                        respond_to_default_color_event(core, event.event, event.terminator);
                    if replacement.is_some() {
                        remove_last_matching_libghostty_color_reply(
                            &mut libghostty_responses,
                            event.event,
                        );
                    }
                    terminal_responses.extend(libghostty_responses);
                    terminal_responses.extend(replacement);
                }
                OrderedPtyResponseEvent::Xtgettcap(response) => {
                    terminal_responses.extend(libghostty_responses);
                    terminal_responses.push(response.bytes);
                }
            }
        }

        if written < bytes.len() {
            core.terminal.write(&bytes[written..]);
            let mut libghostty_responses = self.drain_pending_pty_responses();
            if let Some(event) = in_progress_default_color_event {
                if default_color_event_color(core, event).is_some() {
                    remove_last_matching_libghostty_color_reply(&mut libghostty_responses, event);
                }
            }
            terminal_responses.extend(libghostty_responses);
        }

        if !core.child_default_foreground_changed && !core.child_default_background_changed {
            core.transient_default_color_owner_pgid = None;
        }
    }

    pub(super) fn drain_pending_pty_responses(&self) -> Vec<Bytes> {
        self.pending_pty_responses
            .lock()
            .map(|mut responses| std::mem::take(&mut *responses))
            .unwrap_or_default()
    }

    pub fn seed_history_ansi(&self, ansi: &str) {
        if ansi.is_empty() {
            return;
        }
        let Ok(mut core) = self.core.lock() else {
            return;
        };
        core.kitty_keyboard.observe(ansi.as_bytes());
        core.terminal.write(ansi.as_bytes());
        #[cfg(windows)]
        conpty_recent_cache::update(&mut core);
        if let Ok(mut key_encoder) = self.key_encoder.lock() {
            key_encoder.set_from_terminal(&core.terminal);
        }
    }

    pub fn resize(
        &self,
        rows: u16,
        cols: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Vec<Bytes> {
        if let Ok(mut core) = self.core.lock() {
            let offset_from_bottom = core
                .terminal
                .scrollbar()
                .ok()
                .map(|scrollbar| {
                    scrollbar
                        .total
                        .saturating_sub(scrollbar.offset + scrollbar.len)
                })
                .unwrap_or(0);
            let bottom_before_resize = ghostty_detection_text(&mut core)
                .map(|text| !text.trim().is_empty())
                .unwrap_or(false);
            let resize_recovery_probe_lines = usize::from(rows)
                .saturating_mul(8)
                .max(DEFAULT_DETECTION_ROWS);
            let replay_ansi = if core.terminal.active_screen().ok()
                == Some(crate::terminal::vt::ActiveScreen::Primary)
                && bottom_before_resize
            {
                ghostty_recent_ansi(&mut core, resize_recovery_probe_lines, true)
                    .ok()
                    .filter(|ansi| !ansi.trim().is_empty())
            } else {
                None
            };

            let _ = core
                .terminal
                .resize(cols, rows, cell_width_px, cell_height_px);
            let terminal_responses = self.drain_pending_pty_responses();

            let bottom_is_blank = ghostty_detection_text(&mut core)
                .map(|text| text.trim().is_empty())
                .unwrap_or(false);
            if bottom_is_blank {
                if let Some(ansi) = replay_ansi.as_deref() {
                    core.terminal.scroll_viewport_bottom();
                    core.terminal.write(ansi.as_bytes());
                }
            }
            #[cfg(windows)]
            if core.recent_fallback.usable {
                core.recent_fallback.needs_refresh = true;
                core.terminal.scroll_viewport_bottom();
                conpty_recent_cache::update(&mut core);
            }
            ghostty_set_scroll_offset_from_bottom(&mut core.terminal, offset_from_bottom);
            if offset_from_bottom > 0 {
                let mut remaining = offset_from_bottom.min(resize_recovery_probe_lines);
                while remaining > 0
                    && ghostty_visible_text(&mut core)
                        .map(|text| text.trim().is_empty())
                        .unwrap_or(false)
                {
                    core.terminal.scroll_viewport_delta(1);
                    remaining -= 1;
                }
            }
            terminal_responses
        } else {
            Vec::new()
        }
    }
}

pub(super) fn render_delay_after_pty_write(
    synchronized_output: bool,
    has_kitty_graphics_sequence: bool,
    cursor_position_settle_pending: bool,
    cursor_position_settle_enabled: bool,
) -> Option<Duration> {
    if synchronized_output {
        None
    } else if has_kitty_graphics_sequence {
        Some(KITTY_GRAPHICS_REDRAW_SETTLE)
    } else if cursor_position_settle_enabled && cursor_position_settle_pending {
        Some(CURSOR_POSITION_SETTLE)
    } else {
        None
    }
}

#[derive(Debug)]
enum OrderedPtyResponseEvent {
    DefaultColor(DefaultColorTrackedEvent),
    Xtgettcap(XtgettcapResponse),
}

impl OrderedPtyResponseEvent {
    fn end_offset(&self) -> usize {
        match self {
            Self::DefaultColor(event) => event.end_offset,
            Self::Xtgettcap(response) => response.end_offset,
        }
    }
}

fn contains_kitty_graphics_sequence(bytes: &[u8]) -> bool {
    bytes.windows(3).any(|window| window == b"\x1b_G")
}

fn observe_pty_bytes<'a>(
    core: &mut GhosttyPaneCore,
    pane_id: PaneId,
    shell_pid: u32,
    bytes: &'a [u8],
    foreground_job: impl Fn(u32) -> Option<crate::platform::ForegroundJob>,
) -> (bool, Cow<'a, [u8]>) {
    let _ = core.terminal.take_pwd_changes();
    // Restored history may have exercised terminal callbacks before this live PTY write.
    // Those effects must not be delivered as live pane output.
    let _ = core.terminal.take_bell_count();
    let _ = core.terminal.take_clipboard_writes();
    let default_color_observation = core.default_color_tracker.observe(bytes);
    if shell_pid > 0 && default_color_observation {
        if let Some(owner_pgid) =
            current_transient_default_color_owner(shell_pid, foreground_job(shell_pid).as_ref())
        {
            core.transient_default_color_owner_pgid = Some(owner_pgid);
            debug!(
                pane = pane_id.raw(),
                owner_pgid, "tracked transient default color override"
            );
        }
    }

    core.osc_debug_tracker.observe(bytes);
    for event in core.osc_debug_tracker.drain_pending() {
        debug!(
            pane = pane_id.raw(),
            osc_command = %event.command,
            osc_payload = ?event.payload,
            "agent OSC evidence observed"
        );
    }
    let terminal_title_changed = core.agent_osc_state.observe(bytes);

    let alternate_screen = core
        .terminal
        .active_screen()
        .map(|screen| screen == crate::terminal::vt::ActiveScreen::Alternate)
        .unwrap_or(false);
    let filtered_bytes = if shell_pid > 0 {
        let foreground_job = (!alternate_screen && contains_scrollback_clear_sequence(bytes))
            .then(|| foreground_job(shell_pid))
            .flatten();
        maybe_filter_primary_screen_scrollback_clear(
            bytes,
            alternate_screen,
            foreground_job.as_ref(),
        )
    } else {
        Cow::Borrowed(bytes)
    };
    if filtered_bytes.len() != bytes.len() {
        debug!(
            pane = pane_id.raw(),
            shell_pid, "ignored scrollback clear sequence for droid compatibility"
        );
    }

    (terminal_title_changed, filtered_bytes)
}
