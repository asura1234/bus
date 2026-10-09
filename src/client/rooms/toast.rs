//! Visible notices and time-based toast scrolling/expiry.
use super::{render::display, BusUi};

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
        let maximum_offset = characters.len().saturating_sub(visible);
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
        Some(characters.into_iter().skip(offset).take(visible).collect())
    }

    fn current_notice(&self) -> Option<String> {
        self.visible_error().map(str::to_owned).or_else(|| {
            let room = self.room?;
            let errors = self
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
            (!errors.is_empty()).then_some(errors)
        })
    }

    pub(super) fn sync_toast(&mut self) {
        let notice = self.current_notice();
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
