//! State operations for the messaging state.
use crate::messaging::prefs::colors;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use {
    super::{AgentId, ModelError, Request, RequestId, Room, RoomId},
    crate::messaging::model::RoomAgent,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct BusState {
    pub(super) next_id: u64,
    pub(super) next_status_revision: u64,
    pub(super) rooms: BTreeMap<RoomId, Room>,
    #[serde(deserialize_with = "deserialize_agents")]
    pub(super) agents: BTreeMap<AgentId, RoomAgent>,
    pub(super) requests: BTreeMap<RequestId, Request>,
    pub(super) queues: BTreeMap<AgentId, Vec<RequestId>>,
    pub(super) consumed_callback_ids: BTreeSet<String>,
    pub(super) consumed_provider_turns: BTreeSet<String>,
    /// Provider turns the agent started on its own (a task notification, or the
    /// user typing in its terminal). Their later hooks are activity, never a
    /// reply or an error for the request Bus is waiting on.
    #[serde(default)]
    pub(super) unrelated_provider_turns: BTreeSet<String>,
    pub(super) visible_room: Option<RoomId>,
    /// Dialog fingerprints already spent on an answer; each is single-use.
    #[serde(default)]
    pub(super) consumed_dialog_fingerprints: BTreeSet<String>,
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
            || self.rooms != previous.rooms
            || self.requests != previous.requests
            || self.queues != previous.queues
            || self.consumed_callback_ids != previous.consumed_callback_ids
            || self.consumed_provider_turns != previous.consumed_provider_turns
            || self.unrelated_provider_turns != previous.unrelated_provider_turns
            || self.visible_room != previous.visible_room
            || self.consumed_dialog_fingerprints != previous.consumed_dialog_fingerprints
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
            next_status_revision: 1,
            rooms: BTreeMap::new(),
            agents: BTreeMap::new(),
            requests: BTreeMap::new(),
            queues: BTreeMap::new(),
            consumed_callback_ids: BTreeSet::new(),
            consumed_provider_turns: BTreeSet::new(),
            unrelated_provider_turns: BTreeSet::new(),
            visible_room: None,
            consumed_dialog_fingerprints: BTreeSet::new(),
        }
    }

    pub(crate) fn dialog_fingerprint_consumed(&self, fingerprint: &str) -> bool {
        self.consumed_dialog_fingerprints.contains(fingerprint)
    }

    pub(crate) fn consume_dialog_fingerprint(&mut self, fingerprint: String) {
        self.consumed_dialog_fingerprints.insert(fingerprint);
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
        self.requests.get(&id)
    }

    pub(crate) fn requests(&self) -> impl Iterator<Item = &Request> {
        self.requests.values()
    }
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
