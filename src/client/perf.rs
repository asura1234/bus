//! The client's perf metrics; the recorder writes them to `perf/client.jsonl`.
use super::events::ClientLoopEvent;
use crate::messaging::model::{BusState, RuntimeStatus};
use crate::protocol::wire::FrameData;
use crate::utils::perf::{ContextSample, Field, Gauge, LoopWatch, Timer};
use ratatui::layout::Rect;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// One UI loop iteration: handling an input, server message, resize or timer event.
pub(super) static UI_LOOP: LoopWatch = LoopWatch::new("client.ui_loop");
/// How late the UI timer (animations and autoscroll) fired past its deadline.
pub(super) static TIMER_LAG: Timer = Timer::new("client.timer_lag");
/// Events already queued behind the one being handled.
pub(super) static EVENT_QUEUE: Gauge = Gauge::new("client.event_queue");
/// Building a frame from the client shell state.
pub(super) static COMPOSE: Timer = Timer::new("client.compose");
/// Encoding a frame and writing it to the host terminal.
pub(super) static PRESENT: Timer = Timer::new("client.present");

pub(super) fn event_stage(event: &ClientLoopEvent) -> &'static str {
    match event {
        #[cfg(unix)]
        ClientLoopEvent::StdinInput(_) => "stdin_input",
        #[cfg(unix)]
        ClientLoopEvent::PixelMouse(..) => "pixel_mouse",
        #[cfg(windows)]
        ClientLoopEvent::StdinEvents(_) => "stdin_events",
        ClientLoopEvent::Resize(..) => "resize",
        ClientLoopEvent::TerminalUnavailable(_) => "terminal_unavailable",
        ClientLoopEvent::ServerMessage { .. } => "server_message",
        ClientLoopEvent::ServerDisconnected { .. } => "server_disconnected",
        ClientLoopEvent::Timer => "timer",
    }
}

/// The dev-only live performance overlay (Settings, dev builds only).
pub(super) static OVERLAY: OverlayMeter = OverlayMeter::new();

/// Dev builds and `--dev` sessions offer the overlay in Settings.
pub(super) fn overlay_available() -> bool {
    cfg!(debug_assertions) || std::env::var_os("BUS_DEV").is_some_and(|value| value == "1")
}

const OVERLAY_REFRESH: Duration = Duration::from_secs(1);

/// While off, every call is one relaxed atomic load: no counting, no repaint, no drawing.
pub(super) struct OverlayMeter {
    on: AtomicBool,
    frames: AtomicU64,
    shown: Mutex<OverlayShown>,
}

struct OverlayShown {
    since: Option<Instant>,
    label: String,
}

impl OverlayMeter {
    pub(super) const fn new() -> Self {
        Self {
            on: AtomicBool::new(false),
            frames: AtomicU64::new(0),
            shown: Mutex::new(OverlayShown {
                since: None,
                label: String::new(),
            }),
        }
    }

    pub(super) fn is_on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    pub(super) fn set_on(&self, on: bool, now: Instant, state: Option<&BusState>) {
        self.frames.store(0, Ordering::Relaxed);
        if let Ok(mut shown) = self.shown.lock() {
            shown.since = Some(now);
            shown.label = overlay_label(crate::utils::perf::context(), None, state);
        }
        self.on.store(on, Ordering::Relaxed);
    }

    /// Counts a presented frame for FPS, only while the overlay is on.
    pub(super) fn note_frame(&self) {
        if self.is_on() {
            self.frames.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[cfg(test)]
    fn frames_since_sample(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }

    /// Refreshes the label once a second; true when the overlay needs a repaint.
    pub(super) fn tick(
        &self,
        now: Instant,
        context: Option<ContextSample>,
        state: Option<&BusState>,
    ) -> bool {
        if !self.is_on() {
            return false;
        }
        let Ok(mut shown) = self.shown.lock() else {
            return false;
        };
        let since = *shown.since.get_or_insert(now);
        let elapsed = now.saturating_duration_since(since);
        if elapsed < OVERLAY_REFRESH {
            return false;
        }
        let frames = self.frames.swap(0, Ordering::Relaxed);
        let fps = frames as f64 / elapsed.as_secs_f64();
        shown.label = overlay_label(context, Some(fps), state);
        shown.since = Some(now);
        true
    }

    /// Draws the label over the frame's top-right corner.
    pub(super) fn stamp(&self, frame: &mut FrameData) -> Rect {
        if !self.is_on() {
            return Rect::default();
        }
        let Ok(shown) = self.shown.lock() else {
            return Rect::default();
        };
        let label = if shown.label.is_empty() {
            overlay_label(None, None, None)
        } else {
            shown.label.clone()
        };
        drop(shown);
        let width = usize::from(frame.width);
        let box_width = label
            .lines()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0)
            .min(width);
        let start = width - box_width;
        let area = Rect::new(
            start as u16,
            0,
            box_width as u16,
            label.lines().count().min(usize::from(frame.height)) as u16,
        );
        let fg = crate::protocol::wire::color_to_u32(ratatui::style::Color::Black);
        let bg = crate::protocol::wire::color_to_u32(ratatui::style::Color::Yellow);
        for (row, line) in label.lines().take(usize::from(frame.height)).enumerate() {
            crate::protocol::ansi::clear_truncated_wide_cell(
                &mut frame.cells[row * width..row * width + start],
            );
            let chars: Vec<char> = line.chars().collect();
            let visible = &chars[chars.len().saturating_sub(box_width)..];
            let padding = box_width - visible.len();
            for offset in 0..box_width {
                if let Some(cell) = frame.cells.get_mut(row * width + start + offset) {
                    let ch = if offset < padding {
                        ' '
                    } else {
                        visible[offset - padding]
                    };
                    cell.symbol = ch.to_string();
                    cell.fg = fg;
                    cell.bg = bg;
                    cell.modifier = 0;
                    cell.skip = false;
                    cell.hyperlink = None;
                }
            }
        }
        if let Some(cursor) = frame.cursor.as_mut() {
            if area.contains((cursor.x, cursor.y).into()) {
                cursor.visible = false;
            }
        }
        area
    }
}

pub(super) fn overlay_label(
    context: Option<ContextSample>,
    fps: Option<f64>,
    state: Option<&BusState>,
) -> String {
    let cpu = context
        .map(|context| context.cpu_pct)
        .filter(|cpu| cpu.is_finite());
    let mem = context
        .map(|context| context.rss_mb)
        .filter(|mem| mem.is_finite());
    // Use the same shown status as `bus state`, including dialog overrides.
    let agents = state
        .map(|state| {
            let (running, live) =
                state.agents().fold((0, 0), |(running, live), agent| {
                    match agent.shown_status() {
                        RuntimeStatus::Working => (running + 1, live + 1),
                        RuntimeStatus::Unavailable => (running, live),
                        _ => (running, live + 1),
                    }
                });
            format!("{running} running / {live} live")
        })
        .unwrap_or_else(|| "n/a".into());
    format!(
        " CPU {} MEM {} FPS {} \n AGENTS {agents} ",
        cpu.map_or("n/a".into(), |cpu| format!("{}%", Field(Some(cpu)))),
        mem.map_or("n/a".into(), |mem| format!("{mem:.0}MB")),
        fps.map_or("n/a".into(), |fps| format!("{fps:.0}")),
    )
}

#[cfg(test)]
#[path = "tests/perf_test.rs"]
mod tests;

/// Stable message tags for perf JSONL; no payload inspection is needed.
pub(super) fn message_tag(
    message: &crate::protocol::wire::ServerMessage,
) -> (
    crate::utils::perf::operation::Variant,
    crate::utils::perf::operation::Phase,
) {
    use crate::protocol::wire::ServerMessage as M;
    use crate::utils::perf::operation::{Phase as P, Variant as V};
    match message {
        M::Welcome { .. } => (V::Welcome, P::Control),
        M::Terminal(_) => (V::Terminal, P::TerminalOutput),
        M::Graphics { .. } => (V::Graphics, P::TerminalOutput),
        M::ServerShutdown { .. } => (V::ServerShutdown, P::Control),
        M::Notify { .. } => (V::Notify, P::Effect),
        M::Clipboard { .. } => (V::Clipboard, P::Effect),
        M::WindowTitle { .. } => (V::WindowTitle, P::Effect),
        M::ReloadSoundConfig => (V::ReloadSoundConfig, P::Control),
        M::MouseCapture { .. } => (V::MouseCapture, P::Effect),
        M::TerminalBell { .. } => (V::TerminalBell, P::Effect),
        M::GraphicsFile { .. } => (V::GraphicsFile, P::TerminalOutput),
        M::GraphicsTransmissionRetired { .. } => {
            (V::GraphicsTransmissionRetired, P::TerminalOutput)
        }
        M::ClientShellSnapshot(_) => (V::ClientShellSnapshot, P::Control),
        M::PaneSurface(_) => (V::PaneSurface, P::Surface),
        M::SemanticNotification(_) => (V::SemanticNotification, P::Effect),
        M::ClientShellError { .. } => (V::ClientShellError, P::Control),
        M::ClientShellKeyboardReportAll { .. } => (V::ClientShellKeyboardReportAll, P::Effect),
        M::ClientShellEndpointResponseChunk { .. } => {
            (V::ClientShellEndpointResponseChunk, P::Endpoint)
        }
        M::PaneSurfacePatch(_) => (V::PaneSurfacePatch, P::SurfacePatch),
        M::EndpointControl { .. } => (V::EndpointControl, P::Endpoint),
    }
}
