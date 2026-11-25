//! Newest-first history windows. Older records are requested on scroll.
use super::{
    source::{visible_request, window_signature, Exchange, Exchanges},
    History, LayoutKey, MarkdownSource,
};
use crate::messaging::model::{BusState, Room};
use crate::messaging::model::{RoomHistoryFilters, RoomId};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const PAGE_RECORDS: usize = 256;
// Unfiltered records lay out both sides of each exchange. A smaller initial
// page keeps the first layout bounded with the same scrollback page size.
pub(super) const INITIAL_UNFILTERED_RECORDS: usize = PAGE_RECORDS / 2;

impl History {
    pub(super) fn layout_reusable(&self, key: LayoutKey, now: u64) -> bool {
        self.key.is_some_and(|previous| {
            previous.0 == key.0
                && previous.1 == key.1
                && previous.2 == key.2
                && previous.4 == key.4
                && previous.5 == key.5
                && previous.6 == key.6
                && (previous.3 == key.3 || self.age_valid.contains(&now))
        })
    }

    pub(super) fn prepare_window_source<'a>(
        &mut self,
        state: &'a BusState,
        room: &'a Room,
        revision: u64,
        filters: RoomHistoryFilters,
    ) -> Option<Exchanges<'a>> {
        self.select_window(room.id, filters);
        self.rebuilt = 0;
        self.rendered = 0;
        self.projected = 0;
        let source = (revision, self.record_limit);
        if self.source == Some(source) {
            return None;
        }
        let (exchanges, has_older) = recent_exchanges(state, room, self.record_limit);
        self.has_older = has_older;
        self.signature = if filters == Default::default() {
            let visited = self.room_markdown.len();
            self.room_markdown.retain(|id, _| state.room(*id).is_some());
            if self.room_markdown.len() != visited {
                let shown: BTreeSet<_> = self.room_markdown.values().flatten().copied().collect();
                self.markdown.retain(|source, _| shown.contains(source));
            }
            window_signature(state, &exchanges)
        } else {
            revision
        } ^ (self.record_limit as u64).rotate_left(32);
        self.source = Some(source);
        Some(exchanges)
    }

    pub(super) fn retain_window_markdown(
        &mut self,
        state: &BusState,
        room: RoomId,
        active_markdown: BTreeSet<MarkdownSource>,
        filtered: bool,
    ) {
        if filtered {
            // Keep older warm blocks for lazy scrollback. Pruning every old
            // block on toggle would make the first page scale with all history.
            self.room_markdown
                .entry(room)
                .or_default()
                .extend(active_markdown);
        } else {
            self.room_markdown.insert(room, active_markdown);
            self.room_markdown.retain(|id, _| state.room(*id).is_some());
            let shown: BTreeSet<_> = self.room_markdown.values().flatten().copied().collect();
            self.markdown.retain(|source, _| shown.contains(source));
        }
    }

    pub(super) fn select_window(&mut self, room: RoomId, filters: RoomHistoryFilters) {
        if self.window_room != Some((room, filters)) {
            self.window_room = Some((room, filters));
            self.source = None;
            self.record_limit = if filters == Default::default() {
                INITIAL_UNFILTERED_RECORDS
            } else {
                PAGE_RECORDS
            };
            self.loaded_limit = 0;
            self.has_older = false;
        }
    }

    /// A resize at the live tail can render the newest page without wrapping
    /// older rows the user has already scrolled away from.
    pub(in crate::client::rooms) fn reset_unfiltered_tail(&mut self, room: RoomId) {
        if self.window_room == Some((room, RoomHistoryFilters::default()))
            && self.record_limit > INITIAL_UNFILTERED_RECORDS
        {
            self.record_limit = INITIAL_UNFILTERED_RECORDS;
            self.key = None;
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
) -> (Exchanges<'a>, bool) {
    let _timer = super::phases::GROUPING.start();
    let mut requests = state.recent_room_requests(room.id, u64::MAX).peekable();
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
                        .filter(|request| request.room_id == room.id && visible_request(request))
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
