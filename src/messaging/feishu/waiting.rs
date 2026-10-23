//! Fixed two-second batching of mandatory developer alerts, across every room and sender.
#[cfg(test)]
use crate::messaging::attention::{plain_line, waiting_action};
use crate::messaging::model::{new_developer_notifications, BusState};

const BURST_WINDOW: u64 = 2_000;

#[derive(Debug, Default)]
pub(crate) struct WaitingNotifications {
    allowed: bool,
    pending: Option<Burst>,
}

#[derive(Debug)]
struct Burst {
    deadline: u64,
    lines: Vec<String>,
}

impl WaitingNotifications {
    // Runtime lifecycle control for startup, shutdown and settings-read failures.
    pub(crate) fn configure(&mut self, enabled: bool) {
        self.allowed = enabled;
        if !self.allowed {
            self.pending = None;
        }
    }

    pub(crate) fn observe(&mut self, previous: &BusState, next: &BusState, now: u64) {
        if !self.allowed {
            return;
        }
        for line in new_developer_notifications(previous, next) {
            if let Some(burst) = self.pending.as_mut() {
                burst.lines.push(line);
            } else {
                self.pending = Some(Burst {
                    deadline: now + BURST_WINDOW,
                    lines: vec![line],
                });
            }
        }
    }

    pub(crate) fn take(&mut self, now: u64) -> Option<String> {
        if self
            .pending
            .as_ref()
            .is_none_or(|burst| now < burst.deadline)
        {
            return None;
        }
        Some(self.pending.take()?.lines.join("\n"))
    }
}

#[cfg(test)]
#[path = "tests/waiting_test.rs"]
mod tests;
