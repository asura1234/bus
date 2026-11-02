#[derive(Clone, Default)]
pub struct EventHub {
    inner: std::sync::Arc<EventHubInner>,
}

#[derive(Default)]
struct EventHubInner {
    state: std::sync::Mutex<EventHubState>,
    wake: std::sync::Condvar,
}

#[derive(Default)]
struct EventHubState {
    next_sequence: u64,
    events: Vec<(u64, crate::protocol::api::schema::EventEnvelope)>,
}

impl EventHub {
    const MAX_EVENTS: usize = 512;

    pub fn push(&self, event: crate::protocol::api::schema::EventEnvelope) {
        let Ok(mut state) = self.inner.state.lock() else {
            return;
        };
        state.next_sequence += 1;
        let sequence = state.next_sequence;
        state.events.push((sequence, event));
        let overflow = state.events.len().saturating_sub(Self::MAX_EVENTS);
        if overflow > 0 {
            state.events.drain(0..overflow);
        }
        drop(state);
        self.inner.wake.notify_all();
    }

    pub fn wait_for_change(&self, sequence: u64, timeout: std::time::Duration) -> bool {
        let Ok(state) = self.inner.state.lock() else {
            return false;
        };
        if state.next_sequence != sequence {
            return true;
        }
        let Ok((state, _)) = self.inner.wake.wait_timeout(state, timeout) else {
            return false;
        };
        state.next_sequence != sequence
    }

    pub fn events_after(
        &self,
        sequence: u64,
    ) -> Vec<(u64, crate::protocol::api::schema::EventEnvelope)> {
        let Ok(state) = self.inner.state.lock() else {
            return Vec::new();
        };
        state
            .events
            .iter()
            .filter(|(event_sequence, _)| *event_sequence > sequence)
            .cloned()
            .collect()
    }

    pub fn current_sequence(&self) -> u64 {
        let Ok(state) = self.inner.state.lock() else {
            return 0;
        };
        state.next_sequence
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::api::schema::{EventData, EventEnvelope, EventKind};
    use std::time::Duration;

    fn event() -> EventEnvelope {
        EventEnvelope {
            event: EventKind::PaneUpdated,
            data: EventData::PaneUpdated {
                pane: serde_json::from_value(serde_json::json!({
                    "pane_id": "p1", "terminal_id": "t1", "workspace_id": "w1",
                    "tab_id": "tab1", "focused": false, "agent_status": "idle", "revision": 1
                }))
                .unwrap(),
            },
        }
    }

    #[test]
    fn an_event_between_observation_and_wait_is_not_lost() {
        let hub = EventHub::default();
        let observed = hub.current_sequence();
        hub.push(event());
        assert!(hub.wait_for_change(observed, Duration::ZERO));
    }

    #[test]
    fn a_waiting_reader_wakes_on_push_and_times_out_quietly() {
        let hub = EventHub::default();
        let observed = hub.current_sequence();
        assert!(!hub.wait_for_change(observed, Duration::from_millis(1)));
        let waiter = {
            let hub = hub.clone();
            std::thread::spawn(move || hub.wait_for_change(observed, Duration::from_secs(1)))
        };
        hub.push(event());
        assert!(waiter.join().unwrap());
    }
}
