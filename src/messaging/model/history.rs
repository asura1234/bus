//! Settled requests and room notices that no longer change, kept out of
//! `state.json`. The coordinator appends them to `history.jsonl`; in memory they
//! sit behind one `Arc`, so cloning the state for a save or a UI snapshot copies
//! only the live part.
use super::{AgentId, Prompt, PromptId, Request, RequestId, RoomId};
use std::collections::BTreeMap;
use std::sync::Arc;

/// A settled request or a notice older than this is archived.
pub(crate) const ARCHIVE_AFTER_MS: u64 = 10 * 60 * 1000;
/// Retention for transient routing/dedupe metadata, never message history.
pub(crate) const HISTORY_RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1000;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct History {
    pub(super) requests: BTreeMap<RequestId, Arc<Request>>,
    pub(super) rooms: BTreeMap<RoomId, String>,
    /// Per room, in id order.
    pub(super) notices: BTreeMap<RoomId, BTreeMap<PromptId, Arc<Prompt>>>,
}

/// One archived item, as it is stored on one line of `history.jsonl`.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) enum Archived {
    #[serde(rename = "request")]
    Request(Box<Request>),
    #[serde(rename = "agent_deleted")]
    LegacyAgent {
        id: AgentId,
        #[serde(flatten)]
        agent: super::staffing::RetiredAgent,
    },
    #[serde(rename = "room_deleted")]
    Room { id: RoomId, name: String },
    #[serde(rename = "notice")]
    Notice { room: RoomId, prompt: Prompt },
}

fn settled_at_ms(request: &Request) -> u64 {
    request
        .completed_at_ms
        .unwrap_or(request.prompt.submitted_at_ms)
}

impl History {
    pub(crate) fn len(&self) -> usize {
        self.requests.len()
            + self.notices.values().map(BTreeMap::len).sum::<usize>()
            + self.rooms.len()
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
            .chain(self.rooms.iter().map(|(id, name)| Archived::Room {
                id: *id,
                name: name.clone(),
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
            .chain(
                self.rooms
                    .iter()
                    .filter(|(id, _)| !older.rooms.contains_key(id))
                    .map(|(id, name)| Archived::Room {
                        id: *id,
                        name: name.clone(),
                    }),
            )
    }

    pub(crate) fn contains(&self, item: &Archived) -> bool {
        match item {
            Archived::LegacyAgent { .. } => false,
            Archived::Room { id, .. } => self.rooms.contains_key(id),
            Archived::Request(request) => self.requests.contains_key(&request.id),
            Archived::Notice { room, prompt } => self
                .notices
                .get(room)
                .is_some_and(|notices| notices.contains_key(&prompt.id)),
        }
    }

    pub(crate) fn insert(&mut self, item: Archived) {
        match item {
            Archived::LegacyAgent { .. } => {
                unreachable!("legacy identities must migrate into state before history indexing")
            }
            Archived::Room { id, name } => {
                self.rooms.insert(id, name);
            }
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
    /// Establish the recovery boundary before any routing/event pass can run.
    pub(crate) fn advance_routing_floor(&mut self, now_ms: u64) {
        self.routing_before_ms = self
            .routing_before_ms
            .max(now_ms.saturating_sub(HISTORY_RETENTION_MS));
    }

    /// Moves settled requests and old notices into permanent history and prunes
    /// only transient routing and dedupe metadata. True when anything changed.
    pub(crate) fn archive(&mut self, now_ms: u64) -> bool {
        let before = self.routing_before_ms;
        self.advance_routing_floor(now_ms);
        let pending = self.retain_unrouted_worker_replies() | self.retain_worker_outcomes(now_ms);
        let moved = self.archive_settled(now_ms.saturating_sub(ARCHIVE_AFTER_MS));
        let spent = self.drop_spent_ids(now_ms);
        let routes = self.reply_routes.len();
        let (event_groups, _) = self.worker_pending_sources();
        let retained: std::collections::BTreeSet<_> = self
            .requests()
            .filter(|r| {
                self.routing_source(r)
                    || self.reply_delivery_pending(r)
                    || event_groups.contains(&r.group.unwrap_or(r.id))
            })
            .flat_map(|r| [r.id, r.group.unwrap_or(r.id)])
            .collect();
        self.reply_routes.retain(|id, route| {
            retained.contains(id)
                || route
                    .notification()
                    .is_some_and(|notice| retained.contains(&notice))
        });
        let events = self.prune_worker_events(now_ms);
        pending
            | moved
            | spent
            | events
            | (routes != self.reply_routes.len())
            | (before != self.routing_before_ms)
    }

    pub(super) fn routing_source(&self, request: &Request) -> bool {
        self.event_backfill_source(request) || self.requests.contains_key(&request.id)
    }

    /// Inline sources may still owe live delivery, but that must not resurrect
    /// expired outcome events during upgrade or before the first archive pass.
    pub(super) fn event_backfill_source(&self, request: &Request) -> bool {
        !request.settled() || settled_at_ms(request) >= self.routing_before_ms
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

    /// A deleted room's notices must leave the live room before it is removed.
    pub(super) fn archive_room_notices(&mut self, room: RoomId) {
        if let Some(live) = self.rooms.get_mut(&room) {
            let history = Arc::make_mut(&mut self.history);
            for prompt in live.notices.drain(..) {
                history.insert(Archived::Notice { room, prompt });
            }
        }
    }

    pub(crate) fn notice(&self, id: PromptId) -> Option<&Prompt> {
        self.rooms
            .values()
            .flat_map(|r| &r.notices)
            .find(|p| p.id == id)
            .or_else(|| {
                self.history
                    .notices
                    .values()
                    .find_map(|room| room.get(&id).map(Arc::as_ref))
            })
    }

    pub(crate) fn agent_label(&self, id: AgentId) -> String {
        if let Some(agent) = self.agent(id) {
            return agent.name.clone();
        }
        let name = self
            .retired_agent(id)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| format!("Agent {}", id.0));
        format!("{name} [deleted]")
    }

    pub(crate) fn history_room_name(&self, id: RoomId) -> Option<&str> {
        self.room(id)
            .map(|r| r.name.as_str())
            .or_else(|| self.history.rooms.get(&id).map(String::as_str))
    }

    pub(crate) fn history_rooms(&self) -> impl Iterator<Item = (RoomId, &str)> {
        self.history
            .rooms
            .iter()
            .filter(|(id, _)| self.room(**id).is_none())
            .map(|(id, name)| (*id, name.as_str()))
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
