//! Shared-answer context for a full room or a bounded recent window.
use super::exchange::{explicitly_to_you, visible_request};
use crate::messaging::model::{AgentId, Author, BusState, Prompt, Request, RequestId, RoomId};
use std::collections::BTreeMap;

pub(super) struct Exchange<'a> {
    pub(super) prompt: &'a Prompt,
    pub(super) requests: BTreeMap<AgentId, &'a Request>,
}

pub(super) struct ReplyGroups {
    pub(super) members: BTreeMap<RequestId, Vec<RequestId>>,
    shared_sources: BTreeMap<RequestId, RequestId>,
}

impl ReplyGroups {
    pub(super) fn for_room(state: &BusState, room: RoomId) -> Self {
        Self {
            members: state.groups(),
            shared_sources: state.shared_reply_sources(room, visible_request),
        }
    }

    pub(super) fn for_window(
        state: &BusState,
        exchanges: &BTreeMap<(u64, u64), Exchange<'_>>,
    ) -> Self {
        let mut members: BTreeMap<RequestId, Vec<RequestId>> = BTreeMap::new();
        let mut shared_sources = BTreeMap::new();
        for request in exchanges
            .values()
            .flat_map(|exchange| exchange.requests.values())
            .copied()
        {
            let lead = request.reply_key();
            if lead != request.id {
                members.entry(lead).or_default().push(request.id);
            }
            if request
                .completed_reply
                .as_ref()
                .and_then(|reply| reply.group)
                .is_some()
            {
                shared_sources.entry(lead).or_insert(request.id);
            }
        }
        for (lead, ids) in &mut members {
            if state
                .request(*lead)
                .is_some_and(|request| request.prompt.author == Author::Developer)
            {
                ids.push(*lead);
            }
        }
        Self {
            members,
            shared_sources,
        }
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
        if explicitly_to_you(text) {
            return true;
        }
        let lead = request.reply_key();
        std::iter::once(lead)
            .chain(self.members.get(&lead).into_iter().flatten().copied())
            .filter_map(|id| state.request(id))
            .any(|member| member.prompt.author == Author::Developer)
    }
}
