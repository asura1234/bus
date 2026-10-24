//! Fixed two-second batching of mandatory developer alerts, across every room and sender.
use crate::messaging::model::{developer_alerts, AgentId, BusState};
#[cfg(test)]
use crate::messaging::{
    attention::{plain_line, waiting_action},
    model::new_developer_notifications,
};

const BURST_WINDOW: u64 = 2_000;
/// A repeated ask (same sender and action) is already on the developer's phone.
/// The window lives in memory: after a Bus restart the next copy pages once more.
const REPEAT_WINDOW: u64 = 30 * 60_000;

/// The sender's stable ID, not its display name, so a rename or a reused name
/// neither repeats nor swallows an ask.
type AskKey = (Option<AgentId>, String);

#[derive(Debug, Default)]
pub(crate) struct WaitingNotifications {
    allowed: bool,
    pending: Option<Burst>,
    /// Asks queued, in flight or delivered, by when they were first seen.
    sent: std::collections::BTreeMap<AskKey, u64>,
    in_flight: Vec<AskKey>,
}

#[derive(Debug)]
struct Burst {
    deadline: u64,
    lines: Vec<String>,
    keys: Vec<AskKey>,
}

impl WaitingNotifications {
    // Runtime lifecycle control for startup, shutdown and settings-read failures.
    pub(crate) fn configure(&mut self, enabled: bool) {
        self.allowed = enabled;
        if !self.allowed {
            if let Some(burst) = self.pending.take() {
                self.release(burst.keys);
            }
        }
    }

    /// A failed or cancelled send never reached the developer: a later copy may page.
    pub(crate) fn finish(&mut self, delivered: bool) {
        let keys = std::mem::take(&mut self.in_flight);
        if !delivered {
            self.release(keys);
        }
    }

    fn release(&mut self, keys: Vec<AskKey>) {
        for key in keys {
            self.sent.remove(&key);
        }
    }

    pub(crate) fn observe(&mut self, previous: &BusState, next: &BusState, now: u64) {
        if !self.allowed {
            return;
        }
        self.sent
            .retain(|_, at| now.saturating_sub(*at) < REPEAT_WINDOW);
        for alert in developer_alerts(previous, next, true) {
            let key = (alert.sender, alert.body);
            if self.sent.contains_key(&key) {
                continue;
            }
            self.sent.insert(key.clone(), now);
            let burst = self.pending.get_or_insert_with(|| Burst {
                deadline: now + BURST_WINDOW,
                lines: Vec::new(),
                keys: Vec::new(),
            });
            burst.lines.push(alert.line);
            burst.keys.push(key);
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
        let burst = self.pending.take()?;
        self.in_flight = burst.keys;
        Some(burst.lines.join("\n"))
    }
}

#[cfg(test)]
#[path = "tests/waiting_test.rs"]
mod tests;
