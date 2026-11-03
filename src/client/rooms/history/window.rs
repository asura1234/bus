//! Newest-first filtered history windows. Older records are requested on scroll.
use super::{
    exchange::{explicitly_to_you, visible_request},
    groups::Exchange,
    History,
};
use crate::messaging::model::{Author, BusState, Prompt, Room};
use crate::messaging::model::{RoomHistoryFilters, RoomId};
use std::collections::BTreeMap;

pub(super) const PAGE_RECORDS: usize = 256;

impl History {
    pub(super) fn select_window(&mut self, room: RoomId, filters: RoomHistoryFilters) {
        if self.window_room != Some((room, filters)) {
            self.window_room = Some((room, filters));
            self.record_limit = PAGE_RECORDS;
            self.loaded_limit = 0;
            self.has_older = false;
        }
    }

    /// Keep the first page bounded even for a sparse filter. Once a page is
    /// ready, another batch can fill an empty viewport or extend a scrollback.
    pub(in crate::client::rooms) fn load_older(&mut self) -> bool {
        if self.has_older && self.loaded_limit == self.record_limit {
            self.record_limit = self.record_limit.saturating_add(PAGE_RECORDS);
            self.key = None;
            return true;
        }
        false
    }
}

pub(super) fn recent_exchanges<'a>(
    state: &'a BusState,
    room: &'a Room,
    limit: usize,
) -> (BTreeMap<(u64, u64), Exchange<'a>>, bool) {
    let mut requests = state.recent_requests(u64::MAX).peekable();
    let mut notices = state.notices(room.id).rev().peekable();
    let mut exchanges = BTreeMap::new();
    for _ in 0..limit {
        let take_request = match (requests.peek(), notices.peek()) {
            (Some(request), Some(notice)) => request.id.0 > notice.id.0,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };
        if take_request {
            let Some(request) = requests.next() else {
                break;
            };
            if request.room_id != room.id || !visible_request(request) {
                continue;
            }
            let prompt = &request.prompt;
            exchanges
                .entry((prompt.submitted_at_ms, prompt.id.0))
                .or_insert_with(|| Exchange {
                    prompt,
                    requests: state
                        .requests_for_message(prompt.id)
                        .filter(|request| visible_request(request))
                        .map(|request| (request.agent_id, request))
                        .collect(),
                });
        } else {
            let Some(prompt) = notices.next() else { break };
            exchanges.insert(
                (prompt.submitted_at_ms, prompt.id.0),
                Exchange {
                    prompt,
                    requests: BTreeMap::new(),
                },
            );
        }
    }
    (
        exchanges,
        requests.peek().is_some() || notices.peek().is_some(),
    )
}

pub(super) fn prompt_to_you(prompt: &Prompt) -> bool {
    prompt.author != Author::Developer
        && (prompt.recipient_ids.is_empty()
            || matches!(prompt.author, Author::Agent(_)) && explicitly_to_you(&prompt.text))
}
