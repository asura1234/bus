//! Decides when a Bus room should ding for a new message, like a group chat.
use crate::messaging::model::{Author, BusState, PromptId, Room, RoomId};
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

/// One ding covers a burst of messages arriving within this window.
pub(super) const RING_COOLDOWN: Duration = Duration::from_millis(1500);
/// Snapshots this soon after start or resume replay what happened while Bus was closed.
pub(super) const STARTUP_GRACE: Duration = Duration::from_secs(2);

/// Whether a sound-enabled room gained a message the Human did not write.
#[cfg(test)]
pub(super) fn new_message_should_ring(previous: &BusState, next: &BusState) -> bool {
    ringing_room(previous, next).is_some()
}

/// The first sound-enabled room that gained a message the Human did not
/// write: an agent's final reply, or a prompt authored by an agent (`send
/// --as`). Every new prompt counts, not only each room's latest, because the
/// Human may send right after an agent within one snapshot interval.
pub(super) fn ringing_room(previous: &BusState, next: &BusState) -> Option<RoomId> {
    let rings = |room: RoomId| next.room(room).is_some_and(Room::sound_enabled);
    let new_reply = next
        .rooms()
        .filter(|room| room.sound_enabled())
        .find(|room| {
            let before = previous.room(room.id);
            room.latest_replies.iter().any(|(agent, reply)| {
                before
                    .and_then(|before| before.latest_replies.get(agent))
                    .map(|known| known.request_id)
                    != Some(reply.request_id)
            })
        })
        .map(|room| room.id);
    if new_reply.is_some() {
        return new_reply;
    }
    let known: BTreeSet<PromptId> = previous
        .requests()
        .map(|request| request.prompt.id)
        .chain(
            previous
                .rooms()
                .filter_map(|room| room.latest_prompt.as_ref().map(|prompt| prompt.id)),
        )
        .collect();
    // Dialog notices to orchestrators are delivery-only and never ring.
    next.requests()
        .filter(|request| !request.delivery_only())
        .map(|request| (request.room_id, &request.prompt))
        .chain(
            next.rooms()
                .filter_map(|room| room.latest_prompt.as_ref().map(|prompt| (room.id, prompt))),
        )
        .find(|(room, prompt)| {
            prompt.author != Author::Human && !known.contains(&prompt.id) && rings(*room)
        })
        .map(|(room, _)| room)
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
