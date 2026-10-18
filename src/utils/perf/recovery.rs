//! Recovery-only alert policy. Called by the recorder, never by a blocked UI.
use super::{StallEvent, StallKind};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};

static ENABLED: AtomicBool = AtomicBool::new(true);
static PENDING: Mutex<Option<Vec<u8>>> = Mutex::new(None);
pub(crate) fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Release);
    if !enabled {
        if let Ok(mut pending) = PENDING.lock() {
            *pending = None;
        }
    }
}
pub(crate) fn take() -> Option<Vec<u8>> {
    let mut pending = PENDING.lock().ok()?;
    if ENABLED.load(Ordering::Acquire) {
        pending.take()
    } else {
        *pending = None;
        None
    }
}

#[derive(Default)]
pub(super) struct RecoveryPolicy {
    last_sent: Option<u64>,
}
impl RecoveryPolicy {
    pub(super) fn accept(&mut self, event: &StallEvent, now_us: u64, enabled: bool) -> bool {
        if !enabled
            || event.kind != StallKind::Ended
            || event.loop_name != "client.ui_loop"
            || event.busy_ms <= 3000
            || self
                .last_sent
                .is_some_and(|last| now_us.saturating_sub(last) < 60_000_000)
        {
            return false;
        }
        self.last_sent = Some(now_us);
        true
    }
    /// Called only on the recorder. The UI consumes the single pending alert
    /// at an idle boundary and submits it through the terminal output owner.
    pub(super) fn observe(&mut self, event: &StallEvent, now_us: u64, hour: u8, minute: u8) {
        if self.accept(event, now_us, ENABLED.load(Ordering::Acquire)) {
            if let Ok(mut pending) = PENDING.lock() {
                *pending = Some(encode(event.busy_ms, hour, minute));
            }
        }
    }
}

fn encode(busy_ms: u64, hour: u8, minute: u8) -> Vec<u8> {
    format!(
        "\x1b]9;Bus froze for {:.1} s at {hour:02}:{minute:02}; details: bus perf\x1b\\",
        busy_ms as f64 / 1000.0
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(kind: StallKind, ms: u64) -> StallEvent {
        StallEvent {
            kind,
            loop_name: "client.ui_loop",
            stage: "server_message",
            busy_ms: ms,
            stall_id: 1,
        }
    }
    #[test]
    fn only_recovery_of_a_client_stall_over_three_seconds_alerts() {
        let mut policy = RecoveryPolicy::default();
        assert!(!policy.accept(&event(StallKind::Began, 4000), 1, true));
        assert!(!policy.accept(&event(StallKind::Moved, 5000), 2, true));
        assert!(!policy.accept(&event(StallKind::Ended, 3000), 3, true));
        assert!(!policy.accept(&event(StallKind::Ended, 4000), 4, false));
        let mut server = event(StallKind::Ended, 4000);
        server.loop_name = "server.render_loop";
        assert!(!policy.accept(&server, 5, true));
        assert!(policy.accept(&event(StallKind::Ended, 4000), 6, true));
    }
    #[test]
    fn alerts_are_limited_to_one_per_minute() {
        let mut policy = RecoveryPolicy::default();
        assert!(policy.accept(&event(StallKind::Ended, 3100), 10, true));
        assert!(!policy.accept(&event(StallKind::Ended, 8000), 60_000_009, true));
        assert!(policy.accept(&event(StallKind::Ended, 8000), 60_000_010, true));
    }
    #[test]
    fn formats_the_exact_terminal_notification_without_composer_text() {
        assert_eq!(
            super::encode(3560, 9, 7),
            b"\x1b]9;Bus froze for 3.6 s at 09:07; details: bus perf\x1b\\"
        );
    }
}
