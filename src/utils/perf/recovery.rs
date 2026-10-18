//! Recovery-only alerts, formatted by the recorder and emitted at an idle boundary.
use super::{now_us, StallEvent, StallKind};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Mutex,
};

static ENABLED: AtomicBool = AtomicBool::new(true);
static GENERATION: AtomicU64 = AtomicU64::new(0);
static HAS_PENDING: AtomicBool = AtomicBool::new(false);
static DELIVERY: Mutex<RecoveryQueue> = Mutex::new(RecoveryQueue::new());

pub(crate) fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Release);
    GENERATION.fetch_add(1, Ordering::AcqRel);
    if !enabled {
        if let Ok(mut delivery) = DELIVERY.lock() {
            delivery.pending = None;
            HAS_PENDING.store(false, Ordering::Release);
        }
    }
}
pub(crate) fn has_pending() -> bool {
    HAS_PENDING.load(Ordering::Acquire)
}
pub(crate) fn take() -> Option<Vec<u8>> {
    let mut delivery = DELIVERY.try_lock().ok()?;
    let bytes = delivery.take(now_us(), ENABLED.load(Ordering::Acquire));
    HAS_PENDING.store(false, Ordering::Release);
    bytes
}
/// Called after the output owner has drained the notification and earlier bytes.
pub(crate) fn acknowledged() -> bool {
    let Ok(mut delivery) = DELIVERY.try_lock() else {
        return false;
    };
    delivery.acknowledged(now_us());
    true
}
/// The recorder only supplies completed UI stalls. A blocked output owner keeps
/// one latest candidate; it never accumulates a burst of deferred notifications.
pub(super) fn observe(event: &StallEvent, hour: u8, minute: u8, encode: fn(&str) -> Vec<u8>) {
    if !is_recovery(event) || !ENABLED.load(Ordering::Acquire) {
        return;
    }
    let generation = GENERATION.load(Ordering::Acquire);
    let candidate = Pending {
        event: *event,
        bytes: encode(&message(event.busy_ms, hour, minute)),
    };
    if let Ok(mut delivery) = DELIVERY.lock() {
        if ENABLED.load(Ordering::Acquire) && GENERATION.load(Ordering::Acquire) == generation {
            let previous = delivery.pending.replace(candidate);
            HAS_PENDING.store(true, Ordering::Release);
            drop(delivery);
            drop(previous);
        }
    }
}
/// Reconstruct the stall's wall-clock start using its monotonic identity.
pub(super) fn stall_started_second(wall_ms: u64, age_us: u64) -> i64 {
    ((i128::from(wall_ms) * 1000 - i128::from(age_us)).div_euclid(1_000_000)) as i64
}
fn is_recovery(event: &StallEvent) -> bool {
    event.kind == StallKind::Ended && event.loop_name == "client.ui_loop" && event.busy_ms > 3000
}
struct Pending {
    event: StallEvent,
    bytes: Vec<u8>,
}
struct RecoveryQueue {
    pending: Option<Pending>,
    policy: RecoveryPolicy,
}
impl RecoveryQueue {
    const fn new() -> Self {
        Self {
            pending: None,
            policy: RecoveryPolicy { last_sent: None },
        }
    }
    fn take(&mut self, now_us: u64, enabled: bool) -> Option<Vec<u8>> {
        let pending = self.pending.take()?;
        self.policy
            .accept(&pending.event, now_us, enabled)
            .then_some(pending.bytes)
    }
    fn acknowledged(&mut self, now_us: u64) {
        self.policy.last_sent = Some(now_us);
    }
}
#[derive(Default)]
pub(super) struct RecoveryPolicy {
    last_sent: Option<u64>,
}
impl RecoveryPolicy {
    pub(super) fn accept(&mut self, event: &StallEvent, now_us: u64, enabled: bool) -> bool {
        if !enabled
            || !is_recovery(event)
            || self
                .last_sent
                .is_some_and(|last| now_us.saturating_sub(last) < 60_000_000)
        {
            return false;
        }
        self.last_sent = Some(now_us);
        true
    }
}
fn message(busy_ms: u64, hour: u8, minute: u8) -> String {
    format!(
        "Bus froze for {:.1} s at {hour:02}:{minute:02}; details: bus perf",
        busy_ms as f64 / 1000.0
    )
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
    fn stall_start_clock_stays_in_the_correct_minute_after_delayed_observation() {
        assert_eq!(stall_started_second(63_400, 3_500_000), 59);
        // Another 100 ms of watchdog/logging delay preserves the same origin.
        assert_eq!(stall_started_second(63_500, 3_600_000), 59);
        assert_eq!(stall_started_second(64_000, 4_000_000), 60);
    }
    #[test]
    fn formats_the_exact_terminal_notification_without_composer_text() {
        assert_eq!(
            super::message(3560, 9, 7),
            "Bus froze for 3.6 s at 09:07; details: bus perf"
        );
    }
    #[test]
    fn output_acknowledgement_starts_the_cooldown_after_a_delayed_write() {
        let mut queue = RecoveryQueue::new();
        queue.pending = Some(Pending {
            event: event(StallKind::Ended, 4000),
            bytes: b"first".to_vec(),
        });
        assert!(queue.take(10, true).is_some());
        queue.acknowledged(100_000_000);
        queue.pending = Some(Pending {
            event: event(StallKind::Ended, 5000),
            bytes: b"second".to_vec(),
        });
        assert!(queue.take(120_000_000, true).is_none());
        queue.pending = Some(Pending {
            event: event(StallKind::Ended, 6000),
            bytes: b"third".to_vec(),
        });
        assert!(queue.take(160_000_000, true).is_some());
    }
}
