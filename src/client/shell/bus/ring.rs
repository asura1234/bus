//! Decides when a Bus room should ding for a new message, like a group chat.
use crate::bus::model::*;
use std::time::{Duration, Instant};

/// One ding covers a burst of messages arriving within this window.
pub(super) const RING_COOLDOWN: Duration = Duration::from_millis(1500);
/// Snapshots this soon after start or resume replay what happened while Bus was closed.
pub(super) const STARTUP_GRACE: Duration = Duration::from_secs(2);

/// Whether a sound-enabled room gained a message the Human did not write: an
/// agent's final reply, or a prompt authored by an agent (`send --as`).
pub(super) fn new_message_should_ring(previous: &BusState, next: &BusState) -> bool {
    next.rooms()
        .filter(|room| room.sound_enabled())
        .any(|room| {
            let before = previous.room(room.id);
            let new_reply = room.latest_replies.iter().any(|(agent, reply)| {
                before
                    .and_then(|before| before.latest_replies.get(agent))
                    .map(|known| known.request_id)
                    != Some(reply.request_id)
            });
            let new_agent_prompt = room.latest_prompt.as_ref().is_some_and(|prompt| {
                prompt.author != Author::Human
                    && before
                        .and_then(|before| before.latest_prompt.as_ref())
                        .map(|known| known.id)
                        != Some(prompt.id)
            });
            new_reply || new_agent_prompt
        })
}

/// Rate-limits dings: silent during the startup grace, then one per burst.
#[derive(Debug)]
pub(super) struct Ringer {
    started: Instant,
    last: Option<Instant>,
}

impl Ringer {
    pub(super) fn new(started: Instant) -> Self {
        Self {
            started,
            last: None,
        }
    }

    /// Records and allows a ding at `now`, or refuses it.
    pub(super) fn allow(&mut self, now: Instant) -> bool {
        if now.saturating_duration_since(self.started) < STARTUP_GRACE
            || self
                .last
                .is_some_and(|last| now.saturating_duration_since(last) < RING_COOLDOWN)
        {
            return false;
        }
        self.last = Some(now);
        true
    }
}
