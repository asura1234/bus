//! Newest-first filtered history windows. Older records are requested on scroll.
use super::{
    source::{visible_request, Exchange, Exchanges, Window},
    History, LayoutKey, MarkdownSource,
};
use crate::messaging::model::{BusState, Room};
use crate::messaging::model::{RoomHistoryFilters, RoomId};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const PAGE_RECORDS: usize = 256;

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
        submissions: &BTreeMap<crate::messaging::model::PromptId, super::LocalSendStatus>,
    ) -> Window<'a> {
        self.select_window(room.id, filters);
        let recent = (filters != Default::default())
            .then(|| recent_exchanges(state, room, self.record_limit));
        if let Some((_, has_older)) = &recent {
            self.rebuilt = 0;
            self.rendered = 0;
            self.projected = 0;
            // A filtered revision examines only its bounded newest window.
            self.signature = revision ^ (self.record_limit as u64).rotate_left(32);
            self.has_older = *has_older;
        } else {
            self.prepare_source(state, room, revision, submissions);
            self.has_older = false;
        }
        recent
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
) -> (Exchanges<'a>, bool) {
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
