//! Visible notices and time-based toast scrolling/expiry.
use super::{
    render::{cell_width, display},
    BusUi,
};

const DEFAULT_TOAST_DURATION: std::time::Duration = std::time::Duration::from_secs(15);
const TOAST_EDGE_PAUSE: std::time::Duration = std::time::Duration::from_secs(1);
const TOAST_SCROLL_STEP: std::time::Duration = std::time::Duration::from_millis(125);

#[derive(Clone, Debug)]
pub(super) struct Toast {
    pub(super) message: String,
    pub(super) started_at: std::time::Instant,
    duration: std::time::Duration,
}

impl BusUi {
    pub(super) fn visible_error(&self) -> Option<&str> {
        self.error.as_deref().or_else(|| {
            self.snapshot
                .error
                .as_deref()
                .filter(|error| self.dismissed_snapshot_error.as_deref() != Some(*error))
        })
    }

    pub(super) fn show_toast(&mut self, message: impl Into<String>) {
        self.show_toast_for(message, DEFAULT_TOAST_DURATION);
    }

    pub(super) fn show_toast_for(
        &mut self,
        message: impl Into<String>,
        duration: std::time::Duration,
    ) {
        let message = message.into();
        let now = std::time::Instant::now();
        tracing::debug!(
            event = "bus.toast.shown",
            message_bytes = message.len(),
            duration_ms = duration.as_millis(),
            "Bus toast shown"
        );
        self.toast = Some(Toast {
            message,
            started_at: now,
            duration,
        });
        self.toast_animation_last_tick = Some(now);
    }

    pub(super) fn toast_text_at(&self, now: std::time::Instant, width: u16) -> Option<String> {
        let toast = self.toast.as_ref()?;
        let elapsed = now.saturating_duration_since(toast.started_at);
        if elapsed >= toast.duration || width == 0 {
            return None;
        }
        let message = display(&toast.message);
        let characters = message.chars().collect::<Vec<_>>();
        let visible = usize::from(width);
        // Scroll one character per step, but measure in terminal cells so
        // wide (CJK) text scrolls until its tail fits.
        let mut tail_cells = 0;
        let maximum_offset = characters
            .iter()
            .rposition(|c| {
                tail_cells += cell_width(*c);
                tail_cells > visible
            })
            .map_or(0, |index| (index + 1).min(characters.len() - 1));
        if maximum_offset == 0 {
            return Some(message);
        }
        let pause_ms = TOAST_EDGE_PAUSE.as_millis();
        let step_ms = TOAST_SCROLL_STEP.as_millis();
        let scroll_ms = maximum_offset as u128 * step_ms;
        let cycle_ms = pause_ms * 2 + scroll_ms;
        let position_ms = elapsed.as_millis() % cycle_ms;
        let offset = if position_ms < pause_ms {
            0
        } else if position_ms < pause_ms + scroll_ms {
            ((position_ms - pause_ms) / step_ms) as usize
        } else {
            maximum_offset
        };
        let mut used = 0;
        Some(
            characters
                .into_iter()
                .skip(offset)
                .take_while(|c| {
                    used += cell_width(*c);
                    used <= visible
                })
                .collect(),
        )
    }

    /// Agent notices (such as a turn that ended without a captured reply)
    /// stay in message status and history; the client log records each new
    /// one. Only errors the human must act on become a toast.
    fn agent_notices(&self) -> Option<String> {
        let room = self.room?;
        let notices = self
            .snapshot
            .state
            .agents()
            .filter(|agent| agent.room_id == room)
            .filter_map(|agent| {
                agent
                    .actionable_error
                    .as_ref()
                    .map(|error| format!("{}: {error}", agent.name))
            })
            .collect::<Vec<_>>()
            .join(" · ");
        (!notices.is_empty()).then_some(notices)
    }

    pub(super) fn sync_toast(&mut self) {
        let notices = self.agent_notices();
        if notices != self.observed_agent_notices {
            if let Some(notices) = &notices {
                tracing::info!(event = "bus.agent.notice", notices = %notices, "Agent notice");
            }
            self.observed_agent_notices = notices;
        }
        let notice = self.visible_error().map(str::to_owned);
        if notice == self.observed_notice {
            return;
        }
        self.observed_notice = notice.clone();
        if let Some(notice) = notice {
            self.show_toast(notice);
        }
    }

    pub(super) fn tick_toast(&mut self, now: std::time::Instant) -> bool {
        let Some(toast) = self.toast.as_ref() else {
            self.toast_animation_last_tick = None;
            return false;
        };
        if now.saturating_duration_since(toast.started_at) >= toast.duration {
            self.toast = None;
            self.toast_animation_last_tick = None;
            return true;
        }
        let Some(last_tick) = self.toast_animation_last_tick else {
            self.toast_animation_last_tick = Some(now);
            return false;
        };
        if now.saturating_duration_since(last_tick) < TOAST_SCROLL_STEP {
            return false;
        }
        self.toast_animation_last_tick = Some(now);
        true
    }
}
