//! Source selection and addressing shared by bounded and full history projection.
use crate::messaging::model::{
    AgentId, Author, BusState, Prompt, Request, Room, RoomHistoryFilters,
};
use std::collections::BTreeMap;

pub(super) struct Exchange<'a> {
    pub(super) prompt: &'a Prompt,
    pub(super) requests: BTreeMap<AgentId, &'a Request>,
}
pub(super) type Exchanges<'a> = BTreeMap<(u64, u64), Exchange<'a>>;
pub(super) type Window<'a> = Option<(Exchanges<'a>, bool)>;

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

/// A full projection uses the same Exchange owner as a bounded page.
pub(super) fn exchanges<'a>(state: &'a BusState, room: &'a Room) -> Exchanges<'a> {
    let mut exchanges = BTreeMap::new();
    for request in state
        .requests()
        .filter(|r| r.room_id == room.id && visible_request(r))
    {
        let prompt = &request.prompt;
        exchanges
            .entry((prompt.submitted_at_ms, prompt.id.0))
            .or_insert_with(|| Exchange {
                prompt,
                requests: BTreeMap::new(),
            })
            .requests
            .insert(request.agent_id, request);
    }
    for prompt in state.notices(room.id) {
        exchanges.insert(
            (prompt.submitted_at_ms, prompt.id.0),
            Exchange {
                prompt,
                requests: BTreeMap::new(),
            },
        );
    }
    exchanges
}
