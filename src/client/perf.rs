//! The client's perf metrics; the recorder writes them to `perf/client.jsonl`.
use super::events::ClientLoopEvent;
use crate::utils::perf::{Gauge, LoopWatch, Timer};

/// One UI loop iteration: handling an input, server message, resize or timer event.
pub(super) static UI_LOOP: LoopWatch = LoopWatch::new("client.ui_loop");
/// How late the UI timer (animations, toasts, autoscroll) fired past its deadline.
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
        ClientLoopEvent::ServerDisconnected => "server_disconnected",
        ClientLoopEvent::Timer => "timer",
    }
}
