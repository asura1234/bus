//! State operations for the messaging state.
use super::{Agent, AgentId, ModelError, Request, RequestId, Room, RoomId};
use crate::messaging::prefs::colors;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct BusState {
    pub(super) next_id: u64,
    pub(super) next_status_revision: u64,
    pub(super) rooms: BTreeMap<RoomId, Room>,
    #[serde(deserialize_with = "deserialize_agents")]
    pub(super) agents: BTreeMap<AgentId, Agent>,
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

fn deserialize_agents<'de, D>(deserializer: D) -> Result<BTreeMap<AgentId, Agent>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let mut agents = BTreeMap::<AgentId, Agent>::deserialize(deserializer)?;
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
    agents: &mut BTreeMap<AgentId, Agent>,
    color: fn(&mut Agent) -> &mut [u8; 3],
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

    pub(crate) fn agent(&self, id: AgentId) -> Option<&Agent> {
        self.agents.get(&id)
    }

    pub(crate) fn agents(&self) -> impl Iterator<Item = &Agent> {
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
