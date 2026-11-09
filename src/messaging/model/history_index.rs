//! Derived shared-answer membership, independent of which bodies a view projects.
use super::{Author, Request, RequestId, RoomId};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct HistoryIndex {
    records: BTreeMap<RequestId, Member>,
    developers: BTreeMap<RequestId, usize>,
    followers: BTreeMap<(RoomId, RequestId), BTreeSet<RequestId>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Member {
    key: RequestId,
    room: RoomId,
    developer: bool,
    visible_follower: bool,
}

impl HistoryIndex {
    pub(super) fn insert(&mut self, request: &Request) {
        self.remove(request.id);
        let key = request.reply_key();
        let member = Member {
            key,
            room: request.room_id,
            developer: request.prompt.author == Author::Developer,
            visible_follower: key != request.id && request.visible_in_history(),
        };
        if member.developer {
            *self.developers.entry(key).or_default() += 1;
        }
        if member.visible_follower {
            self.followers
                .entry((member.room, key))
                .or_default()
                .insert(request.id);
        }
        self.records.insert(request.id, member);
    }

    pub(super) fn remove(&mut self, id: RequestId) {
        let Some(member) = self.records.remove(&id) else {
            return;
        };
        if member.developer {
            if let Some(count) = self.developers.get_mut(&member.key) {
                *count -= 1;
                if *count == 0 {
                    self.developers.remove(&member.key);
                }
            }
        }
        if member.visible_follower {
            let key = (member.room, member.key);
            if let Some(ids) = self.followers.get_mut(&key) {
                ids.remove(&id);
                if ids.is_empty() {
                    self.followers.remove(&key);
                }
            }
        }
    }
}

impl Request {
    /// Diagnostic and provider-routing envelopes have no authored chat body.
    pub(crate) fn visible_in_history(&self) -> bool {
        !self.is_error_report()
            && (!self.bus_generated || !matches!(self.prompt.author, Author::Agent(_)))
    }
}
