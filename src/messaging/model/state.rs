//! State operations for the messaging state.
use crate::messaging::prefs::colors;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use {
    super::{history::History, AgentId, ModelError, Prompt, Request, RequestId, Room, RoomId},
    crate::messaging::model::RoomAgent,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct BusState {
    pub(super) next_id: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) room_digests: BTreeMap<AgentId, super::digests::RoomDigest>,
    pub(super) next_status_revision: u64,
    pub(super) rooms: BTreeMap<RoomId, Room>,
    #[serde(deserialize_with = "deserialize_agents")]
    pub(super) agents: BTreeMap<AgentId, RoomAgent>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) retired_agents: BTreeMap<AgentId, super::staffing::RetiredAgent>,
    pub(super) requests: BTreeMap<RequestId, Request>,
    pub(super) queues: BTreeMap<AgentId, Vec<RequestId>>,
    /// Pending, one terminal notification, or seen per shared reply.
    /// Kept outside the immutable request archive so acknowledgements
    /// of older replies are durable without rewriting append-only history.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) reply_routes: BTreeMap<RequestId, super::reply_routing::ReplyRoute>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) worker_events: BTreeMap<String, super::worker_events::WorkerEvent>,
    /// Applied provider callbacks by id, with the launch whose spool they
    /// came from; a re-spooled duplicate is refused while its id is kept.
    /// `None` marks ids carried over from a version 1 file, launch unknown.
    #[serde(default)]
    pub(super) consumed_callbacks: BTreeMap<String, Option<String>>,
    /// Version 1 shapes of the spent-id guards, read once on load and moved
    /// into `consumed_callbacks` and `spent_dialog_answers`; never written.
    #[serde(default, skip_serializing)]
    pub(super) consumed_callback_ids: BTreeSet<String>,
    #[serde(default, skip_serializing)]
    pub(super) consumed_dialog_fingerprints: BTreeSet<String>,
    pub(super) consumed_provider_turns: BTreeSet<String>,
    /// Provider turns the agent started on its own (a task notification, or the
    /// user typing in its terminal). Their later hooks are activity, never a
    /// reply or an error for the request Bus is waiting on.
    #[serde(default)]
    pub(super) unrelated_provider_turns: BTreeSet<String>,
    pub(super) visible_room: Option<RoomId>,
    /// Dialog fingerprints already spent on an answer, by digest, with when
    /// they were observed; each is single-use until it expires.
    #[serde(default)]
    pub(super) spent_dialog_answers: BTreeMap<String, u64>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(super) attention_episodes: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "super::AttentionDelivery::is_empty")]
    pub(super) attention_delivery: super::AttentionDelivery,
    /// Settled requests and notices archived in `history.jsonl`, never in
    /// `state.json`; shared so cloning the state does not copy them.
    #[serde(skip)]
    pub(super) history: Arc<History>,
}

fn deserialize_agents<'de, D>(deserializer: D) -> Result<BTreeMap<AgentId, RoomAgent>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let mut agents = BTreeMap::<AgentId, RoomAgent>::deserialize(deserializer)?;
    backfill_colors(
        &mut agents,
        |agent| &mut agent.color,
        colors::is_agent_color,
        |occupied| colors::next_agent_color(occupied.iter().copied()),
    );
    backfill_colors(
        &mut agents,
        |agent| &mut agent.accessible_color,
        colors::is_accessible_agent_color,
        |occupied| colors::next_accessible_agent_color(occupied.iter().copied()),
    );
    Ok(agents)
}

fn backfill_colors(
    agents: &mut BTreeMap<AgentId, RoomAgent>,
    color: fn(&mut RoomAgent) -> &mut [u8; 3],
    valid: fn([u8; 3]) -> bool,
    next: fn(&[[u8; 3]]) -> [u8; 3],
) {
    let mut room_colors = BTreeMap::<RoomId, Vec<[u8; 3]>>::new();
    // Reserve all saved, readable colors before filling missing legacy fields,
    // including those belonging to agents later in ID order.
    for agent in agents.values_mut() {
        if valid(*color(agent)) {
            room_colors
                .entry(agent.room_id)
                .or_default()
                .push(*color(agent));
        }
    }
    for agent in agents.values_mut() {
        if !valid(*color(agent)) {
            let occupied = room_colors.entry(agent.room_id).or_default();
            *color(agent) = next(occupied);
            occupied.push(*color(agent));
        }
    }
}

impl BusState {
    /// A poll can refresh observation clocks and causal revisions in memory
    /// without rewriting the durable state. Any status edge, settlement, or
    /// other model change still needs an atomic save before delivery resumes.
    pub(crate) fn durable_poll_change_from(&self, previous: &Self) -> bool {
        if self.next_id != previous.next_id
            || self.room_digests != previous.room_digests
            || self.retired_agents != previous.retired_agents
            || self.rooms != previous.rooms
            || self.requests != previous.requests
            || self.queues != previous.queues
            || self.reply_routes != previous.reply_routes
            || self.worker_events != previous.worker_events
            || self.consumed_callbacks != previous.consumed_callbacks
            || self.consumed_provider_turns != previous.consumed_provider_turns
            || self.unrelated_provider_turns != previous.unrelated_provider_turns
            || self.visible_room != previous.visible_room
            || self.spent_dialog_answers != previous.spent_dialog_answers
            || self.attention_episodes != previous.attention_episodes
            || self.attention_delivery != previous.attention_delivery
            || self.history != previous.history
            || self.agents.len() != previous.agents.len()
        {
            return true;
        }
        self.agents.iter().any(|(id, agent)| {
            let Some(before) = previous.agents.get(id) else {
                return true;
            };
            let mut durable = agent.clone();
            durable.status_revision = before.status_revision;
            durable.busy_revision = before.busy_revision;
            durable.observed_at_ms = before.observed_at_ms;
            durable != *before
        })
    }

    pub(crate) fn is_pristine(&self) -> bool {
        self.next_id == 1
    }

    pub(crate) fn new() -> Self {
        Self {
            next_id: 1,
            room_digests: BTreeMap::new(),
            next_status_revision: 1,
            rooms: BTreeMap::new(),
            agents: BTreeMap::new(),
            retired_agents: BTreeMap::new(),
            requests: BTreeMap::new(),
            queues: BTreeMap::new(),
            reply_routes: BTreeMap::new(),
            worker_events: BTreeMap::new(),
            consumed_callbacks: BTreeMap::new(),
            consumed_callback_ids: BTreeSet::new(),
            consumed_dialog_fingerprints: BTreeSet::new(),
            consumed_provider_turns: BTreeSet::new(),
            unrelated_provider_turns: BTreeSet::new(),
            visible_room: None,
            spent_dialog_answers: BTreeMap::new(),
            attention_episodes: BTreeSet::new(),
            attention_delivery: super::AttentionDelivery::default(),
            history: Arc::default(),
        }
    }

    /// Moves guards a version 1 file stored in its own shapes into the
    /// current ones, so what was spent before the upgrade stays spent: callback
    /// ids are kept for good (their launch is unknown), and each fingerprint
    /// counts as observed now, so it stays spent for at least its remaining
    /// lifetime.
    pub(crate) fn adopt_version_one_guards(&mut self, now_ms: u64) {
        for id in std::mem::take(&mut self.consumed_callback_ids) {
            self.consumed_callbacks.entry(id).or_insert(None);
        }
        for token in std::mem::take(&mut self.consumed_dialog_fingerprints) {
            self.spent_dialog_answers
                .entry(fingerprint_digest(&token).to_owned())
                .or_insert(now_ms);
        }
    }

    /// Whether the fingerprint with this digest already answered a dialog.
    pub(crate) fn dialog_answer_spent(&self, digest: &str) -> bool {
        self.spent_dialog_answers.contains_key(digest)
    }

    /// Spends a fingerprint, observed at `observed_at_ms`, on one answer.
    pub(crate) fn spend_dialog_answer(&mut self, digest: String, observed_at_ms: u64) {
        self.spent_dialog_answers.insert(digest, observed_at_ms);
    }

    pub(super) fn allocate_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
    }

    pub(crate) fn room(&self, id: RoomId) -> Option<&Room> {
        self.rooms.get(&id)
    }

    pub(crate) fn rooms(&self) -> impl Iterator<Item = &Room> {
        self.rooms.values()
    }

    pub(crate) fn agent(&self, id: AgentId) -> Option<&RoomAgent> {
        self.agents.get(&id)
    }

    pub(crate) fn agents(&self) -> impl Iterator<Item = &RoomAgent> {
        self.agents.values()
    }

    pub(crate) fn request(&self, id: RequestId) -> Option<&Request> {
        self.requests.get(&id).or_else(|| self.history.request(id))
    }

    /// Live and archived requests, in id order.
    pub(crate) fn requests(&self) -> impl Iterator<Item = &Request> {
        MergeById {
            live: self.requests.values().peekable(),
            archived: self.history.requests.values().map(Arc::as_ref).peekable(),
        }
    }

    /// A room's archived then live notices, in id order.
    pub(crate) fn notices(&self, room: RoomId) -> impl DoubleEndedIterator<Item = &Prompt> {
        self.history.room_notices(room).chain(
            self.rooms
                .get(&room)
                .into_iter()
                .flat_map(|room| room.notices.iter()),
        )
    }

    pub(crate) fn history(&self) -> &Arc<History> {
        &self.history
    }

    /// Settles `id` with `reply`, as a final callback does, and releases its agent.
    #[cfg(test)]
    pub(crate) fn finish_for_test(&mut self, id: RequestId, reply: &str, at_ms: u64) {
        let request = self.requests.get_mut(&id).expect("request");
        request.phase = super::RequestPhase::Completed;
        request.completed_at_ms = Some(at_ms);
        request.pending_final = Some(super::PendingFinal {
            callback_id: format!("final-{}", id.0),
            text: reply.to_owned(),
            received_at_ms: at_ms,
            provider_session_id: None,
            provider_turn_id: None,
        });
        let agent = request.agent_id;
        if let Some(queue) = self.queues.get_mut(&agent) {
            queue.retain(|queued| *queued != id);
        }
        if let Some(agent) = self.agents.get_mut(&agent) {
            if agent.current_request == Some(id) {
                agent.current_request = None;
            }
        }
    }

    /// Attaches the archive a store loaded beside `state.json`.
    pub(crate) fn with_history(mut self, history: Arc<History>) -> Self {
        self.history = history;
        self
    }
}

/// Two id-ordered request iterators merged into one id-ordered iterator.
struct MergeById<'a, L: Iterator<Item = &'a Request>, A: Iterator<Item = &'a Request>> {
    live: std::iter::Peekable<L>,
    archived: std::iter::Peekable<A>,
}

impl<'a, L: Iterator<Item = &'a Request>, A: Iterator<Item = &'a Request>> Iterator
    for MergeById<'a, L, A>
{
    type Item = &'a Request;

    fn next(&mut self) -> Option<&'a Request> {
        match (self.live.peek(), self.archived.peek()) {
            (Some(live), Some(archived)) if archived.id < live.id => self.archived.next(),
            (Some(_), _) => self.live.next(),
            (None, _) => self.archived.next(),
        }
    }
}

/// The digest a dialog fingerprint ends with; it names the fingerprint once spent.
pub(crate) fn fingerprint_digest(fingerprint: &str) -> &str {
    fingerprint.rsplit('.').next().unwrap_or(fingerprint)
}

pub(super) fn normalized_name(name: &str) -> Result<String, ModelError> {
    let name = name.trim();
    if name.is_empty() {
        Err(ModelError::InvalidName)
    } else {
        Ok(name.to_owned())
    }
}

impl Default for BusState {
    fn default() -> Self {
        Self::new()
    }
}
