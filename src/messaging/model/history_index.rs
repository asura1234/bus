//! Derived shared-answer membership, independent of which bodies a view projects.
use super::{Author, Request, RequestId, RoomId};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct HistoryIndex {
    records: BTreeMap<RequestId, (RoomId, RequestId)>,
    developer_replies: BTreeMap<(RoomId, RequestId), BTreeSet<RequestId>>,
}

impl HistoryIndex {
    pub(super) fn insert(&mut self, request: &Request) {
        self.remove(request.id);
        if request.prompt.author != Author::Developer || !request.visible_in_history() {
            return;
        }
        if let Some(group) = request
            .completed_reply
            .as_ref()
            .and_then(|reply| reply.group)
        {
            let key = (request.room_id, group);
            self.developer_replies
                .entry(key)
                .or_default()
                .insert(request.id);
            self.records.insert(request.id, key);
        }
    }

    pub(super) fn remove(&mut self, id: RequestId) {
        let Some(key) = self.records.remove(&id) else {
            return;
        };
        if let Some(ids) = self.developer_replies.get_mut(&key) {
            ids.remove(&id);
            if ids.is_empty() {
                self.developer_replies.remove(&key);
            }
        }
    }

    pub(super) fn first_developer_reply(
        &self,
        room: RoomId,
        group: RequestId,
    ) -> Option<RequestId> {
        self.developer_replies.get(&(room, group))?.first().copied()
    }
}

impl Request {
    /// Diagnostic and provider-routing envelopes have no authored chat body.
    pub(crate) fn visible_in_history(&self) -> bool {
        !self.is_error_report()
            && (!self.bus_generated || !matches!(self.prompt.author, Author::Agent(_)))
    }
}
