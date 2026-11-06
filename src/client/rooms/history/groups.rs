//! Complete group destination context; body placement belongs to the selected page.
use super::source::{exchanges, explicitly_to_you, Exchanges, Window};
use crate::messaging::model::{BusState, Request, RequestId, Room, RoomId};
use std::collections::BTreeMap;

pub(super) struct ReplyGroups {
    shared_sources: BTreeMap<RequestId, RequestId>,
}

impl ReplyGroups {
    pub(super) fn for_projection<'a>(
        state: &'a BusState,
        room: &'a Room,
        recent: Window<'a>,
    ) -> (Self, Exchanges<'a>) {
        let _timer = super::phases::GROUPING.start();
        let exchanges = recent.map_or_else(|| exchanges(state, room), |(entries, _)| entries);
        let mut shared_sources = BTreeMap::new();
        for request in exchanges
            .values()
            .flat_map(|exchange| exchange.requests.values())
        {
            if let Some(group) = request
                .completed_reply
                .as_ref()
                .and_then(|reply| reply.group)
            {
                shared_sources
                    .entry(group)
                    .and_modify(|source: &mut RequestId| *source = (*source).min(request.id))
                    .or_insert(request.id);
            }
        }
        (Self { shared_sources }, exchanges)
    }

    pub(super) fn waiting_elsewhere(
        &self,
        state: &BusState,
        room: RoomId,
        request: &Request,
    ) -> bool {
        request.completed_reply.is_none()
            && state
                .history_latest_group_member(room, request.reply_key())
                .is_some_and(|last| last != request.id)
    }

    pub(super) fn reference(&self, state: &BusState, request: &Request) -> Option<String> {
        let group = request.completed_reply.as_ref()?.group?;
        let source = *self.shared_sources.get(&group)?;
        (source != request.id)
            .then(|| state.request(source))
            .flatten()
            .map(|source| format!("Answered with message {}", source.prompt.id.0))
    }

    pub(super) fn shared_to_developer(
        &self,
        state: &BusState,
        request: &Request,
        text: &str,
    ) -> bool {
        explicitly_to_you(text) || state.history_group_has_developer(request.reply_key())
    }
}
