//! Source records and addressing for history projection.
use crate::messaging::model::{AgentId, Author, BusState, Prompt, Request, RoomHistoryFilters};
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};

pub(super) struct Exchange<'a> {
    pub(super) prompt: &'a Prompt,
    pub(super) requests: BTreeMap<AgentId, &'a Request>,
}
pub(super) type Exchanges<'a> = BTreeMap<(u64, u64), Exchange<'a>>;

#[derive(Clone, Copy)]
pub(super) struct Direction {
    pub(super) to_you: bool,
    pub(super) from_you: bool,
}

impl Direction {
    pub(super) fn prompt(prompt: &Prompt) -> Self {
        Self {
            to_you: prompt.author != Author::Developer
                && (prompt.recipient_ids.is_empty()
                    || matches!(prompt.author, Author::Agent(_))
                        && explicitly_to_you(&prompt.text)),
            from_you: prompt.author == Author::Developer,
        }
    }

    pub(super) fn accepts(self, filters: RoomHistoryFilters) -> bool {
        filters == Default::default()
            || filters.to_you && self.to_you
            || filters.from_you && self.from_you
    }
}

pub(super) fn explicitly_to_you(text: &str) -> bool {
    crate::messaging::attention::has_you_mention(text)
        || crate::messaging::attention::waiting_tag_action(text).is_some()
}

pub(in crate::client::rooms) fn visible_request(request: &Request) -> bool {
    request.visible_in_history()
}

// Prompts are immutable after creation. Check their
// identity/settlement metadata, not their potentially large text, when a
// global poll/draft revision arrives. Only a changed room history, names,
// width, or age label requires sorting and wrapping those immutable bodies.
pub(super) fn window_signature(state: &BusState, exchanges: &Exchanges<'_>) -> u64 {
    let _timer = super::phases::SIGNATURE.start();
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    let mut agents = BTreeSet::new();
    for exchange in exchanges.values() {
        let prompt = exchange.prompt;
        prompt.id.0.hash(&mut hash);
        prompt.submitted_at_ms.hash(&mut hash);
        agents.extend(prompt.recipient_ids.iter().copied());
        if let Author::Agent(id) = prompt.author {
            agents.insert(id);
        }
        for request in exchange.requests.values() {
            request.id.0.hash(&mut hash);
            request.group.map(|lead| lead.0).hash(&mut hash);
            request.history_send_mode().hash(&mut hash);
            request.history_mode_marker().hash(&mut hash);
            let outcome = request
                .group
                .and_then(|lead| state.request(lead))
                .unwrap_or(request);
            outcome.uncertain_outcome.hash(&mut hash);
            outcome.failure_reason.hash(&mut hash);
            outcome.submission_error.hash(&mut hash);
            if let Some(reply) = state.developer_history_reply(request) {
                reply.received_at_ms.hash(&mut hash);
                reply.callback_id.hash(&mut hash);
            }
        }
    }
    // Headers can name a participant from another room, such as a MASTER
    // orchestrator. Only owners of visible exchanges can affect this layout.
    for id in agents {
        id.0.hash(&mut hash);
        state
            .agent(id)
            .map(|agent| {
                (
                    &agent.name,
                    agent.color,
                    &agent.cwd,
                    agent.room_id.0,
                    agent.orchestrates.map(|room| room.0),
                )
            })
            .hash(&mut hash);
    }
    hash.finish()
}
