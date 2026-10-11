//! Rooms operations for the messaging state.
use super::state::normalized_name;
use super::types::{ReportTurn, MASTER_ROOM_ID};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};
use {
    super::{
        AgentId, AgentRecipients, Author, BusState, Draft, ModelError, Prompt, PromptId, Room,
        RoomId, RoomKind, MASTER_ROOM_NAME,
    },
    crate::messaging::model::RoomAgent,
};

impl BusState {
    pub(crate) fn create_room(&mut self, name: &str) -> Result<RoomId, ModelError> {
        let name = work_room_name(name)?;
        let id = RoomId(self.allocate_id());
        self.rooms.insert(
            id,
            Room {
                id,
                name,
                notices: Vec::new(),
                notes: String::new(),
                draft: Draft::default(),
                unread_count: 0,
                latest_prompt: None,
                latest_replies: BTreeMap::new(),
                deletion_pending: false,
                kind: RoomKind::Work,
                sound: None,
                sound_name: None,
                report_turns: BTreeMap::new(),
            },
        );
        Ok(id)
    }

    /// Gives a session its MASTER room if it lacks one, and drops orchestrator
    /// assignments that no longer satisfy the MASTER invariants.
    pub(crate) fn ensure_master_room(&mut self) -> RoomId {
        // Sessions saved before MASTER accepted any name; keep its name unique.
        let clashing: Vec<_> = self
            .rooms
            .values()
            .filter(|room| {
                room.kind == RoomKind::Work && room.name.eq_ignore_ascii_case(MASTER_ROOM_NAME)
            })
            .map(|room| room.id)
            .collect();
        for id in clashing {
            let name = (1..)
                .map(|n| match n {
                    1 => "Master (old)".to_owned(),
                    n => format!("Master (old {n})"),
                })
                .find(|name| self.rooms.values().all(|room| &room.name != name))
                .expect("an unused name exists");
            if let Some(room) = self.rooms.get_mut(&id) {
                room.name = name;
            }
        }
        let master = match self.master_room() {
            Some(room) => room.id,
            None => {
                self.rooms.insert(
                    MASTER_ROOM_ID,
                    Room {
                        id: MASTER_ROOM_ID,
                        name: MASTER_ROOM_NAME.into(),
                        notices: Vec::new(),
                        notes: String::new(),
                        draft: Draft::default(),
                        unread_count: 0,
                        latest_prompt: None,
                        latest_replies: BTreeMap::new(),
                        deletion_pending: false,
                        kind: RoomKind::Master,
                        sound: None,
                        sound_name: None,
                        report_turns: BTreeMap::new(),
                    },
                );
                MASTER_ROOM_ID
            }
        };
        let mut claimed = BTreeSet::new();
        for agent in self.agents.values_mut() {
            let valid = agent.orchestrates.is_some_and(|room| {
                agent.room_id == master
                    && self
                        .rooms
                        .get(&room)
                        .is_some_and(|room| room.kind == RoomKind::Work)
                    && claimed.insert(room)
            });
            if !valid {
                agent.orchestrates = None;
            }
        }
        master
    }

    /// Whether the session holds anything beyond its MASTER room.
    pub(crate) fn has_work(&self) -> bool {
        !self.agents.is_empty() || self.rooms.values().any(|room| room.kind == RoomKind::Work)
    }

    pub(crate) fn master_room(&self) -> Option<&Room> {
        self.rooms
            .values()
            .find(|room| room.kind == RoomKind::Master)
    }

    /// The MASTER agent orchestrating `room`, if any.
    pub(crate) fn orchestrator_of(&self, room: RoomId) -> Option<&RoomAgent> {
        self.agents
            .values()
            .find(|agent| agent.orchestrates == Some(room))
    }

    /// Binds a new MASTER agent to the work room it orchestrates. The room is
    /// fixed in the agent's system prompt at launch, so it is bound once, at
    /// creation, and never moves.
    pub(crate) fn bind_orchestrator(
        &mut self,
        id: AgentId,
        room: RoomId,
    ) -> Result<(), ModelError> {
        let agent = self.agents.get(&id).ok_or(ModelError::UnknownAgent(id))?;
        if self
            .rooms
            .get(&agent.room_id)
            .is_none_or(|home| home.kind != RoomKind::Master)
        {
            return Err(ModelError::OrchestratorOutsideMaster(id));
        }
        if agent.orchestrates.is_some() {
            return Err(ModelError::OrchestratorAlreadyBound(id));
        }
        if self
            .rooms
            .get(&room)
            .is_none_or(|target| target.kind != RoomKind::Work || target.deletion_pending)
        {
            return Err(ModelError::NotOrchestratable(room));
        }
        if let Some(other) = self.orchestrator_of(room) {
            return Err(ModelError::RoomAlreadyOrchestrated {
                room,
                agent: other.id,
            });
        }
        self.agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?
            .orchestrates = Some(room);
        Ok(())
    }

    /// The agents a room's deletion removes: its members and its orchestrator,
    /// which exists only for that room.
    pub(crate) fn agents_deleted_with_room(&self, id: RoomId) -> Vec<AgentId> {
        self.agents
            .values()
            .filter(|agent| agent.room_id == id || agent.orchestrates == Some(id))
            .map(|agent| agent.id)
            .collect()
    }

    pub(crate) fn set_room_sound(&mut self, id: RoomId, on: bool) -> Result<(), ModelError> {
        self.rooms
            .get_mut(&id)
            .ok_or(ModelError::UnknownRoom(id))?
            .sound = Some(on);
        Ok(())
    }

    /// `name` is a system sound name, or None for Bus's own ding.
    pub(crate) fn set_room_sound_name(
        &mut self,
        id: RoomId,
        name: Option<String>,
    ) -> Result<(), ModelError> {
        self.rooms
            .get_mut(&id)
            .ok_or(ModelError::UnknownRoom(id))?
            .sound_name = name;
        Ok(())
    }

    pub(crate) fn rename_room(&mut self, id: RoomId, name: &str) -> Result<(), ModelError> {
        let name = work_room_name(name)?;
        let room = self.rooms.get_mut(&id).ok_or(ModelError::UnknownRoom(id))?;
        if room.kind == RoomKind::Master {
            return Err(ModelError::MasterRoomFixed);
        }
        room.name = name;
        Ok(())
    }

    pub(crate) fn prepare_delete_room(&mut self, id: RoomId) -> Result<(), ModelError> {
        let room = self.rooms.get_mut(&id).ok_or(ModelError::UnknownRoom(id))?;
        if room.kind == RoomKind::Master {
            return Err(ModelError::MasterRoomFixed);
        }
        room.deletion_pending = true;
        for agent in self.agents_deleted_with_room(id) {
            self.prepare_delete_agent(agent)?;
        }
        Ok(())
    }

    pub(crate) fn delete_room(&mut self, id: RoomId) -> Result<(), ModelError> {
        match self.rooms.get(&id) {
            None => return Err(ModelError::UnknownRoom(id)),
            Some(room) if room.kind == RoomKind::Master => return Err(ModelError::MasterRoomFixed),
            Some(_) => {}
        }
        for agent in self.agents_deleted_with_room(id) {
            self.delete_agent(agent)?;
        }
        self.rooms.remove(&id);
        self.requests.retain(|_, request| request.room_id != id);
        // Match the UI, which falls back to the first room (MASTER) at once,
        // so `state` never shows a gap between the delete and the next view.
        if self.visible_room == Some(id) {
            self.visible_room = self.rooms.keys().next().copied();
        }
        Ok(())
    }

    pub(crate) fn set_room_notes(&mut self, id: RoomId, notes: &str) -> Result<(), ModelError> {
        let room = self.rooms.get_mut(&id).ok_or(ModelError::UnknownRoom(id))?;
        // Notes are a work room's status board; MASTER is where the developer talks to
        // orchestrators and has no board of its own.
        if room.kind == RoomKind::Master {
            return Err(ModelError::MasterRoomHasNoNotes);
        }
        room.notes = notes.to_owned();
        Ok(())
    }

    pub(crate) fn set_draft_text(&mut self, id: RoomId, text: &str) -> Result<(), ModelError> {
        self.rooms
            .get_mut(&id)
            .ok_or(ModelError::UnknownRoom(id))?
            .draft
            .text = text.to_owned();
        Ok(())
    }

    pub(crate) fn set_draft_recipients(
        &mut self,
        id: RoomId,
        recipients: impl IntoIterator<Item = AgentId>,
    ) -> Result<(), ModelError> {
        if !self.rooms.contains_key(&id) {
            return Err(ModelError::UnknownRoom(id));
        }
        let recipients = recipients.into_iter().collect::<AgentRecipients>();
        for agent_id in &recipients {
            let agent = self
                .agents
                .get(agent_id)
                .ok_or(ModelError::UnknownAgent(*agent_id))?;
            if agent.room_id != id {
                return Err(ModelError::AgentOutsideRoom(*agent_id));
            }
        }
        self.rooms
            .get_mut(&id)
            .ok_or(ModelError::UnknownRoom(id))?
            .draft
            .recipient_ids = recipients;
        Ok(())
    }

    pub(crate) fn attach_file(&mut self, room: RoomId, path: PathBuf) -> Result<(), ModelError> {
        let files = &mut self
            .rooms
            .get_mut(&room)
            .ok_or(ModelError::UnknownRoom(room))?
            .draft
            .files;
        if !files.contains(&path) {
            files.push(path);
        }
        Ok(())
    }

    pub(crate) fn remove_file(
        &mut self,
        room: RoomId,
        path: &std::path::Path,
    ) -> Result<(), ModelError> {
        self.rooms
            .get_mut(&room)
            .ok_or(ModelError::UnknownRoom(room))?
            .draft
            .files
            .retain(|candidate| candidate != path);
        Ok(())
    }

    /// Posts an agent's message to the developer in `room`, delivered to no agent.
    /// Orchestrators report this way when no developer message is open, so the
    /// report reaches the room's history instead of only their terminal.
    pub(crate) fn post_to_developer(
        &mut self,
        room: RoomId,
        author: AgentId,
        text: String,
        files: Vec<PathBuf>,
        now_ms: u64,
    ) -> Result<PromptId, ModelError> {
        if text.trim().is_empty() && files.is_empty() {
            return Err(ModelError::EmptyPrompt);
        }
        let agent = self
            .agents
            .get(&author)
            .ok_or(ModelError::UnknownAgent(author))?;
        if agent.room_id != room {
            return Err(ModelError::AgentOutsideRoom(author));
        }
        if agent.deletion_pending {
            return Err(ModelError::DeletionPending);
        }
        let id = PromptId(self.allocate_id());
        let visible = self.visible_room == Some(room);
        let room = self
            .rooms
            .get_mut(&room)
            .ok_or(ModelError::UnknownRoom(room))?;
        let prompt = Prompt {
            id,
            author: Author::Agent(author),
            text,
            files,
            recipient_ids: AgentRecipients::default(),
            submitted_at_ms: now_ms,
            compaction_limit_notice: false,
        };
        // The latest prompt is what rings and what the composer recalls, as
        // for an agent's `send --as` to other agents.
        room.latest_prompt = Some(prompt.clone());
        room.notices.push(prompt);
        if !visible {
            room.unread_count = room.unread_count.saturating_add(1);
        }
        Ok(id)
    }

    /// The first notice id `agent`'s provider turn `turn` (session and turn
    /// id) could have posted in `room`: one after its previous turn's last
    /// final. Before any recorded turn, or for a turn without an id, 0.
    pub(crate) fn report_turn_start(
        &self,
        room: RoomId,
        agent: AgentId,
        turn: Option<(&Option<String>, &str)>,
    ) -> u64 {
        match (self.rooms.get(&room), turn) {
            (Some(room), Some(turn)) => report_turn_floor(room, agent, turn),
            _ => 0,
        }
    }

    /// Records that `agent`'s provider turn `turn` reached a final, so a later
    /// turn's report is not compared with what this one posted.
    pub(crate) fn end_report_turn(
        &mut self,
        room: RoomId,
        agent: AgentId,
        session: Option<String>,
        turn: String,
    ) {
        let next_notice = self.next_id;
        let Some(room) = self.rooms.get_mut(&room) else {
            return;
        };
        let first_notice = report_turn_floor(room, agent, (&session, &turn));
        room.report_turns.insert(
            agent,
            ReportTurn {
                session,
                turn,
                first_notice,
                next_notice,
            },
        );
    }

    pub(crate) fn select_room(&mut self, room: RoomId) -> Result<(), ModelError> {
        self.mark_room_seen(room)?;
        self.visible_room = Some(room);
        Ok(())
    }

    pub(crate) fn visible_room(&self) -> Option<RoomId> {
        self.visible_room
    }

    pub(crate) fn leave_room_view(&mut self) {
        self.visible_room = None;
    }

    pub(crate) fn mark_room_seen(&mut self, room: RoomId) -> Result<(), ModelError> {
        self.rooms
            .get_mut(&room)
            .ok_or(ModelError::UnknownRoom(room))?
            .unread_count = 0;
        Ok(())
    }
}

fn report_turn_floor(room: &Room, agent: AgentId, (session, turn): (&Option<String>, &str)) -> u64 {
    room.report_turns.get(&agent).map_or(0, |last| {
        if &last.session == session && last.turn == turn {
            last.first_notice
        } else {
            last.next_notice
        }
    })
}

fn work_room_name(name: &str) -> Result<String, ModelError> {
    let name = normalized_name(name)?;
    if name.eq_ignore_ascii_case(MASTER_ROOM_NAME) {
        return Err(ModelError::ReservedRoomName);
    }
    Ok(name)
}
