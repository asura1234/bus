//! Settled requests and room notices that no longer change, kept out of
//! `state.json`. The coordinator appends them to `history.jsonl`; in memory they
//! sit behind one `Arc`, so cloning the state for a save or a UI snapshot copies
//! only the live part.
use super::{Prompt, PromptId, Request, RequestId, RoomId};
use std::collections::BTreeMap;
use std::sync::Arc;

/// A settled request or a notice older than this is archived.
pub(crate) const ARCHIVE_AFTER_MS: u64 = 10 * 60 * 1000;
/// Archived history older than this is dropped.
pub(crate) const HISTORY_RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1000;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct History {
    pub(super) requests: BTreeMap<RequestId, Arc<Request>>,
    /// Per room, in id order.
    pub(super) notices: BTreeMap<RoomId, BTreeMap<PromptId, Arc<Prompt>>>,
}

/// One archived item, as it is stored on one line of `history.jsonl`.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) enum Archived {
    #[serde(rename = "request")]
    Request(Box<Request>),
    #[serde(rename = "notice")]
    Notice { room: RoomId, prompt: Prompt },
}

impl Archived {
    /// When the item settled, which retention counts from: a request's
    /// reply (or its send time when it closed without one), a notice's post.
    pub(crate) fn settled_at_ms(&self) -> u64 {
        match self {
            Self::Request(request) => settled_at_ms(request),
            Self::Notice { prompt, .. } => prompt.submitted_at_ms,
        }
    }

    pub(crate) fn room(&self) -> RoomId {
        match self {
            Self::Request(request) => request.room_id,
            Self::Notice { room, .. } => *room,
        }
    }
}

fn settled_at_ms(request: &Request) -> u64 {
    request
        .completed_at_ms
        .unwrap_or(request.prompt.submitted_at_ms)
}

impl History {
    pub(crate) fn len(&self) -> usize {
        self.requests.len() + self.notices.values().map(BTreeMap::len).sum::<usize>()
    }

    /// Every item, requests first, each in id order.
    pub(crate) fn items(&self) -> impl Iterator<Item = Archived> + '_ {
        self.requests
            .values()
            .map(|request| Archived::Request(Box::new(Request::clone(request))))
            .chain(self.notices.iter().flat_map(|(room, notices)| {
                notices.values().map(|prompt| Archived::Notice {
                    room: *room,
                    prompt: Prompt::clone(prompt),
                })
            }))
    }

    /// The items of `self` that `older` lacks: what an append must add.
    pub(crate) fn added_since<'a>(
        &'a self,
        older: &'a Self,
    ) -> impl Iterator<Item = Archived> + 'a {
        self.requests
            .iter()
            .filter(|(id, _)| !older.requests.contains_key(id))
            .map(|(_, request)| Archived::Request(Box::new(Request::clone(request))))
            .chain(self.notices.iter().flat_map(move |(room, notices)| {
                notices
                    .iter()
                    .filter(move |(id, _)| {
                        !older
                            .notices
                            .get(room)
                            .is_some_and(|known| known.contains_key(id))
                    })
                    .map(move |(_, prompt)| Archived::Notice {
                        room: *room,
                        prompt: Prompt::clone(prompt),
                    })
            }))
    }

    pub(crate) fn contains(&self, item: &Archived) -> bool {
        match item {
            Archived::Request(request) => self.requests.contains_key(&request.id),
            Archived::Notice { room, prompt } => self
                .notices
                .get(room)
                .is_some_and(|notices| notices.contains_key(&prompt.id)),
        }
    }

    pub(crate) fn insert(&mut self, item: Archived) {
        match item {
            Archived::Request(request) => {
                self.requests.insert(request.id, Arc::new(*request));
            }
            Archived::Notice { room, prompt } => {
                self.notices
                    .entry(room)
                    .or_default()
                    .insert(prompt.id, Arc::new(prompt));
            }
        }
    }

    pub(super) fn request(&self, id: RequestId) -> Option<&Request> {
        self.requests.get(&id).map(Arc::as_ref)
    }

    pub(super) fn room_notices(&self, room: RoomId) -> impl DoubleEndedIterator<Item = &Prompt> {
        self.notices
            .get(&room)
            .into_iter()
            .flat_map(|notices| notices.values().map(Arc::as_ref))
    }
}

/// A dialog fingerprint answers only within this long of its observation.
pub(crate) const DIALOG_ANSWER_TTL_MS: u64 = 60 * 60 * 1000;

impl super::BusState {
    /// Moves settled requests and old notices into the archive and drops what
    /// no longer needs keeping. True when anything changed.
    pub(crate) fn archive(&mut self, now_ms: u64) -> bool {
        let moved = self.archive_settled(now_ms.saturating_sub(ARCHIVE_AFTER_MS));
        let expired = self.drop_history_before(now_ms.saturating_sub(HISTORY_RETENTION_MS));
        let spent = self.drop_spent_ids(now_ms);
        let routes = self.reply_routes.len();
        let retained: std::collections::BTreeSet<_> = self.requests().map(|r| r.id).collect();
        self.reply_routes.retain(|id, notice| {
            retained.contains(id) || notice.is_some_and(|notice| retained.contains(&notice))
        });
        moved | expired | spent | (routes != self.reply_routes.len())
    }

    /// Settled requests no agent, queue or open group refers to, and notices,
    /// once `cutoff` is past them.
    fn archive_settled(&mut self, cutoff: u64) -> bool {
        let referenced: std::collections::BTreeSet<RequestId> = self
            .agents
            .values()
            .filter_map(|agent| agent.current_request)
            .chain(self.queues.values().flatten().copied())
            .collect();
        let open_groups: std::collections::BTreeSet<RequestId> = self
            .requests
            .values()
            .filter(|request| !request.settled())
            .map(|request| request.group.unwrap_or(request.id))
            .collect();
        let requests: Vec<RequestId> = self
            .requests
            .values()
            .filter(|request| {
                request.settled()
                    && settled_at_ms(request) <= cutoff
                    && !referenced.contains(&request.id)
                    && !open_groups.contains(&request.group.unwrap_or(request.id))
            })
            .map(|request| request.id)
            .collect();
        let notices: Vec<(RoomId, usize)> = self
            .rooms
            .values()
            .map(|room| {
                let old = room
                    .notices
                    .iter()
                    .take_while(|notice| notice.submitted_at_ms <= cutoff)
                    .count();
                (room.id, old)
            })
            .filter(|(_, old)| *old > 0)
            .collect();
        if requests.is_empty() && notices.is_empty() {
            return false;
        }
        let history = Arc::make_mut(&mut self.history);
        for id in requests {
            if let Some(request) = self.requests.remove(&id) {
                history.insert(Archived::Request(Box::new(request)));
            }
        }
        for (room, old) in notices {
            if let Some(live) = self.rooms.get_mut(&room) {
                for prompt in live.notices.drain(..old) {
                    history.insert(Archived::Notice { room, prompt });
                }
            }
        }
        true
    }

    fn drop_history_before(&mut self, cutoff: u64) -> bool {
        // A pending or queued wake still owns its original reply, including the full text
        // behind a long-reply pointer. Ordinary history retention must not erase
        // it while the orchestrator's terminal remains unavailable.
        let pinned: std::collections::BTreeSet<_> = self
            .requests()
            .filter(|r| self.reply_delivery_pending(r))
            .map(|r| r.id)
            .collect();
        self.drop_history_where(|_, _, at, id| {
            at < cutoff && id.is_none_or(|id| !pinned.contains(&id))
        })
    }

    /// Drops a deleted room's archived requests and notices.
    pub(super) fn drop_history_of_room(&mut self, room: RoomId) {
        self.drop_history_where(|of, _, _, _| of == room);
    }

    /// Drops a deleted agent's archived requests.
    pub(super) fn drop_history_of_agent(&mut self, agent: super::AgentId) {
        self.drop_history_where(|_, of, _, _| of == Some(agent));
    }

    /// Drops archived items for which `drop(room, request agent, settled at)` holds.
    fn drop_history_where(
        &mut self,
        drop: impl Fn(RoomId, Option<super::AgentId>, u64, Option<RequestId>) -> bool,
    ) -> bool {
        let requests: Vec<RequestId> = self
            .history
            .requests
            .values()
            .filter(|request| {
                drop(
                    request.room_id,
                    Some(request.agent_id),
                    settled_at_ms(request),
                    Some(request.id),
                )
            })
            .map(|request| request.id)
            .collect();
        let notices: Vec<(RoomId, PromptId)> = self
            .history
            .notices
            .iter()
            .flat_map(|(room, notices)| {
                notices
                    .values()
                    .filter(|prompt| drop(*room, None, prompt.submitted_at_ms, None))
                    .map(|prompt| (*room, prompt.id))
            })
            .collect();
        if requests.is_empty() && notices.is_empty() {
            return false;
        }
        let history = Arc::make_mut(&mut self.history);
        for id in requests {
            history.requests.remove(&id);
        }
        for (room, id) in notices {
            if let Some(room_notices) = history.notices.get_mut(&room) {
                room_notices.remove(&id);
                if room_notices.is_empty() {
                    history.notices.remove(&room);
                }
            }
        }
        true
    }

    /// Spent fingerprints past their lifetime, and callback ids and provider
    /// turns of launches no agent runs any more, can never match again:
    /// callbacks are consumed only from an agent's current launch. Keeping them
    /// would only grow every save.
    fn drop_spent_ids(&mut self, now_ms: u64) -> bool {
        let answers = self.spent_dialog_answers.len();
        self.spent_dialog_answers.retain(|_, observed_at_ms| {
            now_ms.saturating_sub(*observed_at_ms) <= DIALOG_ANSWER_TTL_MS
        });
        let launches: std::collections::BTreeSet<String> = self
            .agents
            .values()
            .filter_map(|agent| agent.runtime_identity.launch_id.clone())
            .collect();
        let callbacks = self.consumed_callbacks.len();
        self.consumed_callbacks.retain(|_, launch| {
            launch
                .as_ref()
                .is_none_or(|launch| launches.contains(launch))
        });
        let live = |key: &String| {
            key.split('\0')
                .next()
                .is_some_and(|launch| launches.contains(launch))
        };
        let turns = self.consumed_provider_turns.len() + self.unrelated_provider_turns.len();
        let consumed: std::collections::BTreeSet<String> = self
            .consumed_provider_turns
            .iter()
            .filter(|key| live(key))
            .cloned()
            .collect();
        let unrelated: std::collections::BTreeSet<String> = self
            .unrelated_provider_turns
            .iter()
            .filter(|key| live(key))
            .cloned()
            .collect();
        let changed = answers != self.spent_dialog_answers.len()
            || callbacks != self.consumed_callbacks.len()
            || turns != consumed.len() + unrelated.len();
        self.consumed_provider_turns = consumed;
        self.unrelated_provider_turns = unrelated;
        changed
    }
}
