use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub(crate) struct $name(pub(crate) u64);
    };
}

id_type!(RoomId);
id_type!(AgentId);
id_type!(PromptId);
id_type!(RequestId);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Provider {
    Codex,
    ClaudeCode,
    Cursor,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RuntimeStatus {
    Launching,
    Idle,
    Working,
    Blocked,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RequestPhase {
    Queued,
    Submitting,
    Active,
    Completed,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct AgentRuntimeIdentity {
    pub(crate) launch_id: Option<String>,
    pub(crate) terminal_id: Option<String>,
    pub(crate) pane_id: Option<String>,
    pub(crate) session_id: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Draft {
    pub(crate) text: String,
    pub(crate) files: Vec<PathBuf>,
    pub(crate) recipient_ids: BTreeSet<AgentId>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Prompt {
    pub(crate) id: PromptId,
    pub(crate) text: String,
    pub(crate) files: Vec<PathBuf>,
    pub(crate) recipient_ids: BTreeSet<AgentId>,
    pub(crate) submitted_at_ms: u64,
}

impl Prompt {
    pub(crate) fn rendered_payload(&self) -> String {
        // Terminal paste sources can preserve CRLF or lone CR separators while
        // provider submit hooks report the accepted prompt with LF separators.
        // Canonicalize only line endings so trusted callback matching remains
        // exact for every other character.
        let text = self.text.replace("\r\n", "\n").replace('\r', "\n");
        let quoted_files = self
            .files
            .iter()
            .map(|path| {
                let path = path.to_string_lossy();
                format!("\"{}\"", path.replace('\\', "\\\\").replace('"', "\\\""))
            })
            .collect::<Vec<_>>()
            .join(" ");
        match (text.is_empty(), quoted_files.is_empty()) {
            (false, false) => format!("{text}\n{quoted_files}"),
            (false, true) => text,
            (true, false) => quoted_files,
            (true, true) => String::new(),
        }
    }

    fn matches_callback_payload(&self, payload: &str) -> bool {
        fn normalize(value: &str) -> String {
            value
                .replace("\r\n", "\n")
                .replace('\r', "\n")
                .trim_end_matches(|character: char| character.is_ascii_whitespace())
                .to_owned()
        }

        normalize(payload) == normalize(&self.rendered_payload())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Reply {
    pub(crate) request_id: RequestId,
    pub(crate) agent_id: AgentId,
    pub(crate) text: String,
    pub(crate) received_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Room {
    pub(crate) id: RoomId,
    pub(crate) name: String,
    pub(crate) notes: String,
    pub(crate) draft: Draft,
    pub(crate) unread_count: u64,
    pub(crate) latest_prompt: Option<Prompt>,
    pub(crate) latest_replies: BTreeMap<AgentId, Reply>,
    #[serde(default)]
    pub(crate) deletion_pending: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Agent {
    pub(crate) id: AgentId,
    pub(crate) room_id: RoomId,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) color: [u8; 3],
    /// Shown instead of `color` in color blind mode; allocated the same way.
    #[serde(default)]
    pub(crate) accessible_color: [u8; 3],
    pub(crate) provider: Provider,
    pub(crate) cwd: PathBuf,
    pub(crate) branch: Option<String>,
    pub(crate) runtime_identity: AgentRuntimeIdentity,
    pub(crate) details_disclosed: bool,
    pub(crate) status: RuntimeStatus,
    pub(crate) status_revision: u64,
    pub(crate) actionable_error: Option<String>,
    pub(crate) current_request: Option<RequestId>,
    #[serde(default)]
    pub(crate) hook_setup_confirmed: bool,
    #[serde(default)]
    pub(crate) session_binding_invalidated: bool,
    #[serde(default)]
    pub(crate) deletion_pending: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct PendingFinal {
    pub(crate) callback_id: String,
    pub(crate) text: String,
    pub(crate) received_at_ms: u64,
    pub(crate) provider_session_id: Option<String>,
    pub(crate) provider_turn_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Request {
    pub(crate) id: RequestId,
    pub(crate) room_id: RoomId,
    pub(crate) agent_id: AgentId,
    pub(crate) prompt: Prompt,
    pub(crate) phase: RequestPhase,
    pub(crate) expected_launch_id: Option<String>,
    pub(crate) submission_boundary: Option<u64>,
    pub(crate) submission_status_revision: u64,
    pub(crate) provider_session_id: Option<String>,
    pub(crate) provider_turn_id: Option<String>,
    pub(crate) provider_prompt_id: Option<String>,
    #[serde(default)]
    pub(crate) trusted_start_bound: bool,
    pub(crate) uncertain_outcome: bool,
    pub(crate) pending_final: Option<PendingFinal>,
    pub(crate) completed_at_ms: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SubmissionOutcome {
    DefinitelyRejected {
        message: String,
    },
    Confirmed {
        provider_session_id: Option<String>,
        provider_turn_id: Option<String>,
    },
    Uncertain {
        message: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum CallbackEventKind {
    PromptStarted,
    Final { text: String },
    Error { message: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ProviderCallback {
    pub(crate) callback_id: String,
    pub(crate) sequence: u64,
    pub(crate) occurred_at_ms: u64,
    pub(crate) agent_id: AgentId,
    pub(crate) launch_id: String,
    pub(crate) provider_session_id: Option<String>,
    pub(crate) provider_turn_id: Option<String>,
    pub(crate) provider_prompt_id: Option<String>,
    pub(crate) prompt_payload: Option<String>,
    pub(crate) kind: CallbackEventKind,
}

impl ProviderCallback {
    #[cfg(test)]
    // Callback fixtures intentionally expose every correlation field at the call site.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn final_event(
        callback_id: &str,
        sequence: u64,
        agent_id: AgentId,
        launch_id: &str,
        provider_session_id: &str,
        provider_turn_id: &str,
        prompt_payload: &str,
        text: &str,
    ) -> Self {
        Self {
            callback_id: callback_id.into(),
            sequence,
            occurred_at_ms: sequence,
            agent_id,
            launch_id: launch_id.into(),
            provider_session_id: Some(provider_session_id.into()),
            provider_turn_id: Some(provider_turn_id.into()),
            provider_prompt_id: None,
            prompt_payload: Some(prompt_payload.into()),
            kind: CallbackEventKind::Final { text: text.into() },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CallbackRejection {
    DuplicateCallback,
    NoActiveRequest,
    WrongLaunch,
    BeforeSubmissionBoundary,
    WrongSession,
    WrongTurn,
    WrongPrompt,
    UnboundFinal,
    DuplicateFinal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CallbackDisposition {
    AcceptedBinding,
    AcceptedPendingSettlement,
    AcceptedCompleted,
    AcceptedError,
    Rejected(CallbackRejection),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ModelError {
    InvalidName,
    UnknownRoom(RoomId),
    UnknownAgent(AgentId),
    UnknownRequest(RequestId),
    AgentOutsideRoom(AgentId),
    EmptyPrompt,
    NoRecipients,
    InvalidTransition,
    MissingLaunchIdentity,
    LaunchIdentityMismatch,
    DeletionPending,
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Bus model operation failed: {self:?}")
    }
}

impl std::error::Error for ModelError {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct BusState {
    next_id: u64,
    next_status_revision: u64,
    rooms: BTreeMap<RoomId, Room>,
    #[serde(deserialize_with = "deserialize_agents")]
    agents: BTreeMap<AgentId, Agent>,
    requests: BTreeMap<RequestId, Request>,
    queues: BTreeMap<AgentId, Vec<RequestId>>,
    consumed_callback_ids: BTreeSet<String>,
    consumed_provider_turns: BTreeSet<String>,
    visible_room: Option<RoomId>,
}

fn deserialize_agents<'de, D>(deserializer: D) -> Result<BTreeMap<AgentId, Agent>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let mut agents = BTreeMap::<AgentId, Agent>::deserialize(deserializer)?;
    backfill_colors(
        &mut agents,
        |agent| &mut agent.color,
        super::colors::is_agent_color,
        |occupied| super::colors::next_agent_color(occupied.iter().copied()),
    );
    backfill_colors(
        &mut agents,
        |agent| &mut agent.accessible_color,
        super::colors::is_accessible_agent_color,
        |occupied| super::colors::next_accessible_agent_color(occupied.iter().copied()),
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
            visible_room: None,
        }
    }

    fn allocate_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
    }

    pub(crate) fn create_room(&mut self, name: &str) -> Result<RoomId, ModelError> {
        let name = normalized_name(name)?;
        let id = RoomId(self.allocate_id());
        self.rooms.insert(
            id,
            Room {
                id,
                name,
                notes: String::new(),
                draft: Draft::default(),
                unread_count: 0,
                latest_prompt: None,
                latest_replies: BTreeMap::new(),
                deletion_pending: false,
            },
        );
        Ok(id)
    }

    pub(crate) fn rename_room(&mut self, id: RoomId, name: &str) -> Result<(), ModelError> {
        let name = normalized_name(name)?;
        self.rooms
            .get_mut(&id)
            .ok_or(ModelError::UnknownRoom(id))?
            .name = name;
        Ok(())
    }

    pub(crate) fn create_agent(
        &mut self,
        room_id: RoomId,
        name: &str,
        provider: Provider,
        cwd: PathBuf,
        branch: Option<String>,
    ) -> Result<AgentId, ModelError> {
        match self.rooms.get(&room_id) {
            None => return Err(ModelError::UnknownRoom(room_id)),
            Some(room) if room.deletion_pending => return Err(ModelError::DeletionPending),
            Some(_) => {}
        }
        let name = normalized_name(name)?;
        let neighbors = || {
            self.agents
                .values()
                .filter(|agent| agent.room_id == room_id)
        };
        let color = super::colors::next_agent_color(neighbors().map(|agent| agent.color));
        let accessible_color = super::colors::next_accessible_agent_color(
            neighbors().map(|agent| agent.accessible_color),
        );
        let id = AgentId(self.allocate_id());
        self.agents.insert(
            id,
            Agent {
                id,
                room_id,
                name,
                color,
                accessible_color,
                provider,
                cwd,
                branch,
                runtime_identity: AgentRuntimeIdentity::default(),
                details_disclosed: false,
                status: RuntimeStatus::Launching,
                status_revision: 0,
                actionable_error: None,
                current_request: None,
                hook_setup_confirmed: false,
                session_binding_invalidated: false,
                deletion_pending: false,
            },
        );
        self.queues.insert(id, Vec::new());
        Ok(id)
    }

    pub(crate) fn rename_agent(&mut self, id: AgentId, name: &str) -> Result<(), ModelError> {
        let name = normalized_name(name)?;
        self.agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?
            .name = name;
        Ok(())
    }

    /// Suspend delivery durably before the runtime attempts terminal shutdown.
    pub(crate) fn prepare_delete_agent(&mut self, id: AgentId) -> Result<(), ModelError> {
        let agent = self
            .agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?;
        agent.deletion_pending = true;
        agent.actionable_error = Some(
            "Deletion pending; queued messages are suspended. Retry deletion to finish.".into(),
        );
        Ok(())
    }

    pub(crate) fn prepare_delete_room(&mut self, id: RoomId) -> Result<(), ModelError> {
        self.rooms
            .get_mut(&id)
            .ok_or(ModelError::UnknownRoom(id))?
            .deletion_pending = true;
        let agents: Vec<_> = self
            .agents
            .values()
            .filter(|agent| agent.room_id == id)
            .map(|agent| agent.id)
            .collect();
        for agent in agents {
            self.prepare_delete_agent(agent)?;
        }
        Ok(())
    }

    pub(crate) fn delete_agent(&mut self, id: AgentId) -> Result<(), ModelError> {
        self.agents
            .remove(&id)
            .ok_or(ModelError::UnknownAgent(id))?;
        self.queues.remove(&id);
        self.requests.retain(|_, request| request.agent_id != id);
        for room in self.rooms.values_mut() {
            room.draft.recipient_ids.remove(&id);
            room.latest_replies.remove(&id);
        }
        Ok(())
    }

    pub(crate) fn delete_room(&mut self, id: RoomId) -> Result<(), ModelError> {
        if !self.rooms.contains_key(&id) {
            return Err(ModelError::UnknownRoom(id));
        }
        let agents: Vec<_> = self
            .agents
            .values()
            .filter(|agent| agent.room_id == id)
            .map(|agent| agent.id)
            .collect();
        for agent in agents {
            self.delete_agent(agent)?;
        }
        self.rooms.remove(&id);
        self.requests.retain(|_, request| request.room_id != id);
        if self.visible_room == Some(id) {
            self.visible_room = None;
        }
        Ok(())
    }

    pub(crate) fn set_agent_runtime_identity(
        &mut self,
        id: AgentId,
        identity: AgentRuntimeIdentity,
    ) -> Result<(), ModelError> {
        self.agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?
            .runtime_identity = identity;
        Ok(())
    }

    pub(crate) fn confirm_hook_setup(&mut self, id: AgentId) -> Result<(), ModelError> {
        let agent = self
            .agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?;
        if agent.session_binding_invalidated {
            return Err(ModelError::InvalidTransition);
        }
        agent.hook_setup_confirmed = true;
        Ok(())
    }

    pub(crate) fn invalidate_agent_session(&mut self, id: AgentId) -> Result<(), ModelError> {
        let agent = self
            .agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?;
        agent.session_binding_invalidated = true;
        agent.hook_setup_confirmed = false;
        Ok(())
    }

    pub(crate) fn set_agent_details_disclosed(
        &mut self,
        id: AgentId,
        disclosed: bool,
    ) -> Result<(), ModelError> {
        self.agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?
            .details_disclosed = disclosed;
        Ok(())
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

    pub(crate) fn update_agent_runtime_metadata(
        &mut self,
        id: AgentId,
        cwd: PathBuf,
        branch: Option<String>,
    ) -> Result<(), ModelError> {
        let agent = self
            .agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?;
        agent.cwd = cwd;
        agent.branch = branch;
        Ok(())
    }

    pub(crate) fn set_agent_error(
        &mut self,
        id: AgentId,
        error: Option<String>,
    ) -> Result<(), ModelError> {
        self.agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?
            .actionable_error = error;
        Ok(())
    }

    pub(crate) fn set_room_notes(&mut self, id: RoomId, notes: &str) -> Result<(), ModelError> {
        self.rooms
            .get_mut(&id)
            .ok_or(ModelError::UnknownRoom(id))?
            .notes = notes.to_owned();
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
        let recipients = recipients.into_iter().collect::<BTreeSet<_>>();
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

    pub(crate) fn quote_reply(
        &mut self,
        room: RoomId,
        agent: AgentId,
        text: &str,
    ) -> Result<(), ModelError> {
        let agent = self
            .agents
            .get(&agent)
            .ok_or(ModelError::UnknownAgent(agent))?;
        if agent.room_id != room {
            return Err(ModelError::AgentOutsideRoom(agent.id));
        }
        let draft = &mut self
            .rooms
            .get_mut(&room)
            .ok_or(ModelError::UnknownRoom(room))?
            .draft;
        if !draft.text.is_empty() && !draft.text.ends_with('\n') {
            draft.text.push('\n');
        }
        let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
        draft.text.push_str(&agent.name);
        draft.text.push_str(": \"");
        draft.text.push_str(&escaped);
        draft.text.push_str("\"\n");
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

    /// Queue an automation prompt without changing the room's human-owned draft.
    pub(crate) fn submit_message(
        &mut self,
        room: RoomId,
        draft: Draft,
        now_ms: u64,
    ) -> Result<Vec<RequestId>, ModelError> {
        let original = self
            .rooms
            .get(&room)
            .ok_or(ModelError::UnknownRoom(room))?
            .draft
            .clone();
        self.rooms
            .get_mut(&room)
            .ok_or(ModelError::UnknownRoom(room))?
            .draft = draft;
        let result = self.submit_draft(room, now_ms);
        self.rooms
            .get_mut(&room)
            .ok_or(ModelError::UnknownRoom(room))?
            .draft = original;
        result
    }

    pub(crate) fn submit_draft(
        &mut self,
        room: RoomId,
        now_ms: u64,
    ) -> Result<Vec<RequestId>, ModelError> {
        let room_state = self.rooms.get(&room).ok_or(ModelError::UnknownRoom(room))?;
        if room_state.deletion_pending {
            return Err(ModelError::DeletionPending);
        }
        let draft = room_state.draft.clone();
        if draft.text.is_empty() && draft.files.is_empty() {
            return Err(ModelError::EmptyPrompt);
        }
        if draft.recipient_ids.is_empty() {
            return Err(ModelError::NoRecipients);
        }
        for agent_id in &draft.recipient_ids {
            let agent = self
                .agents
                .get(agent_id)
                .ok_or(ModelError::UnknownAgent(*agent_id))?;
            if agent.room_id != room {
                return Err(ModelError::AgentOutsideRoom(*agent_id));
            }
            if agent.deletion_pending {
                return Err(ModelError::DeletionPending);
            }
        }

        let prompt = Prompt {
            id: PromptId(self.allocate_id()),
            text: draft.text,
            files: draft.files,
            recipient_ids: draft.recipient_ids,
            submitted_at_ms: now_ms,
        };
        let mut request_ids = Vec::with_capacity(prompt.recipient_ids.len());
        for agent_id in prompt.recipient_ids.iter().copied() {
            let request_id = RequestId(self.allocate_id());
            self.requests.insert(
                request_id,
                Request {
                    id: request_id,
                    room_id: room,
                    agent_id,
                    prompt: prompt.clone(),
                    phase: RequestPhase::Queued,
                    expected_launch_id: None,
                    submission_boundary: None,
                    submission_status_revision: 0,
                    provider_session_id: None,
                    provider_turn_id: None,
                    provider_prompt_id: None,
                    trusted_start_bound: false,
                    uncertain_outcome: false,
                    pending_final: None,
                    completed_at_ms: None,
                },
            );
            self.queues.entry(agent_id).or_default().push(request_id);
            request_ids.push(request_id);
        }
        let room_state = self
            .rooms
            .get_mut(&room)
            .ok_or(ModelError::UnknownRoom(room))?;
        room_state.latest_prompt = Some(prompt);
        room_state.draft.text.clear();
        room_state.draft.files.clear();
        Ok(request_ids)
    }

    pub(crate) fn begin_submission(
        &mut self,
        request: RequestId,
        launch_id: &str,
        callback_boundary: u64,
    ) -> Result<(), ModelError> {
        let request_state = self
            .requests
            .get(&request)
            .ok_or(ModelError::UnknownRequest(request))?;
        if request_state.phase != RequestPhase::Queued {
            return Err(ModelError::InvalidTransition);
        }
        let agent_id = request_state.agent_id;
        let agent = self
            .agents
            .get(&agent_id)
            .ok_or(ModelError::UnknownAgent(agent_id))?;
        if agent.deletion_pending {
            return Err(ModelError::DeletionPending);
        }
        let expected_launch = agent
            .runtime_identity
            .launch_id
            .as_deref()
            .ok_or(ModelError::MissingLaunchIdentity)?;
        if expected_launch != launch_id {
            return Err(ModelError::LaunchIdentityMismatch);
        }
        let submission_status_revision = agent.status_revision;
        if agent.current_request.is_some()
            || self
                .queues
                .get(&agent_id)
                .and_then(|queue| queue.first())
                .copied()
                != Some(request)
        {
            return Err(ModelError::InvalidTransition);
        }

        let queue = self
            .queues
            .get_mut(&agent_id)
            .ok_or(ModelError::InvalidTransition)?;
        queue.remove(0);
        self.agents
            .get_mut(&agent_id)
            .ok_or(ModelError::UnknownAgent(agent_id))?
            .current_request = Some(request);
        let request_state = self
            .requests
            .get_mut(&request)
            .ok_or(ModelError::UnknownRequest(request))?;
        request_state.phase = RequestPhase::Submitting;
        request_state.expected_launch_id = Some(launch_id.to_owned());
        request_state.submission_boundary = Some(callback_boundary);
        request_state.submission_status_revision = submission_status_revision;
        Ok(())
    }

    pub(crate) fn record_submission(
        &mut self,
        request: RequestId,
        outcome: SubmissionOutcome,
    ) -> Result<(), ModelError> {
        let agent_id = self
            .requests
            .get(&request)
            .ok_or(ModelError::UnknownRequest(request))?
            .agent_id;
        let request_state = self
            .requests
            .get_mut(&request)
            .ok_or(ModelError::UnknownRequest(request))?;
        if request_state.phase != RequestPhase::Submitting {
            return Err(ModelError::InvalidTransition);
        }
        match outcome {
            SubmissionOutcome::DefinitelyRejected { message } => {
                if request_state.trusted_start_bound {
                    return Err(ModelError::InvalidTransition);
                }
                request_state.phase = RequestPhase::Queued;
                request_state.expected_launch_id = None;
                request_state.submission_boundary = None;
                self.queues.entry(agent_id).or_default().insert(0, request);
                let agent = self
                    .agents
                    .get_mut(&agent_id)
                    .ok_or(ModelError::UnknownAgent(agent_id))?;
                agent.current_request = None;
                agent.actionable_error = Some(message);
            }
            SubmissionOutcome::Confirmed {
                provider_session_id,
                provider_turn_id,
            } => {
                request_state.phase = RequestPhase::Active;
                request_state.provider_session_id = provider_session_id;
                request_state.provider_turn_id = provider_turn_id;
                request_state.uncertain_outcome = false;
            }
            SubmissionOutcome::Uncertain { message } => {
                request_state.uncertain_outcome = true;
                self.agents
                    .get_mut(&agent_id)
                    .ok_or(ModelError::UnknownAgent(agent_id))?
                    .actionable_error = Some(message);
            }
        }
        Ok(())
    }

    pub(crate) fn observe_status(
        &mut self,
        agent: AgentId,
        status: RuntimeStatus,
        now_ms: u64,
    ) -> Result<(), ModelError> {
        let revision = self.next_status_revision;
        self.next_status_revision = self.next_status_revision.saturating_add(1);
        let agent_state = self
            .agents
            .get_mut(&agent)
            .ok_or(ModelError::UnknownAgent(agent))?;
        agent_state.status = status;
        agent_state.status_revision = revision;
        if status == RuntimeStatus::Idle {
            self.complete_pending_final(agent, now_ms)?;
        }
        Ok(())
    }

    pub(crate) fn accept_callback(&mut self, callback: ProviderCallback) -> CallbackDisposition {
        if !self
            .consumed_callback_ids
            .insert(callback.callback_id.clone())
        {
            return CallbackDisposition::Rejected(CallbackRejection::DuplicateCallback);
        }
        let Some(agent) = self.agents.get(&callback.agent_id) else {
            return CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest);
        };
        if agent.deletion_pending {
            return CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest);
        }
        let Some(request_id) = agent.current_request else {
            return CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest);
        };
        let Some(request) = self.requests.get(&request_id) else {
            return CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest);
        };
        let settled_after_submission = agent.status == RuntimeStatus::Idle
            && agent.status_revision > request.submission_status_revision;
        if request.expected_launch_id.as_deref() != Some(callback.launch_id.as_str()) {
            return CallbackDisposition::Rejected(CallbackRejection::WrongLaunch);
        }
        if request
            .submission_boundary
            .is_some_and(|boundary| callback.sequence <= boundary)
        {
            return CallbackDisposition::Rejected(CallbackRejection::BeforeSubmissionBoundary);
        }
        if let Some(expected) = request.provider_session_id.as_deref() {
            if callback.provider_session_id.as_deref() != Some(expected) {
                return CallbackDisposition::Rejected(CallbackRejection::WrongSession);
            }
        }
        if let Some(expected) = request.provider_turn_id.as_deref() {
            if callback.provider_turn_id.as_deref() != Some(expected) {
                return CallbackDisposition::Rejected(CallbackRejection::WrongTurn);
            }
        }
        if let Some(expected) = request.provider_prompt_id.as_deref() {
            if callback
                .provider_prompt_id
                .as_deref()
                .is_some_and(|actual| actual != expected)
            {
                return CallbackDisposition::Rejected(CallbackRejection::WrongPrompt);
            }
        }
        if callback
            .prompt_payload
            .as_deref()
            .is_some_and(|payload| !request.prompt.matches_callback_payload(payload))
        {
            return CallbackDisposition::Rejected(CallbackRejection::WrongPrompt);
        }

        match callback.kind {
            CallbackEventKind::PromptStarted => {
                if callback.provider_session_id.is_none() || callback.provider_turn_id.is_none() {
                    return CallbackDisposition::Rejected(CallbackRejection::WrongPrompt);
                }
                if !callback
                    .prompt_payload
                    .as_deref()
                    .is_some_and(|payload| request.prompt.matches_callback_payload(payload))
                {
                    return CallbackDisposition::Rejected(CallbackRejection::WrongPrompt);
                }
                let Some(request) = self.requests.get_mut(&request_id) else {
                    return CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest);
                };
                request.provider_session_id = callback.provider_session_id;
                request.provider_turn_id = callback.provider_turn_id;
                request.provider_prompt_id = callback.provider_prompt_id;
                request.trusted_start_bound = true;
                request.phase = RequestPhase::Active;
                request.uncertain_outcome = false;
                if let Some(agent) = self.agents.get_mut(&callback.agent_id) {
                    agent.actionable_error = None;
                }
                CallbackDisposition::AcceptedBinding
            }
            CallbackEventKind::Final { text } => {
                if !request.trusted_start_bound
                    || request.provider_session_id.is_none()
                    || request.provider_turn_id.is_none()
                {
                    return CallbackDisposition::Rejected(CallbackRejection::UnboundFinal);
                }
                if request.pending_final.is_some() {
                    return CallbackDisposition::Rejected(CallbackRejection::DuplicateFinal);
                }
                let turn_key = provider_turn_key(
                    &callback.launch_id,
                    callback.provider_session_id.as_deref(),
                    callback.provider_turn_id.as_deref(),
                );
                if turn_key
                    .as_ref()
                    .is_some_and(|key| self.consumed_provider_turns.contains(key))
                {
                    return CallbackDisposition::Rejected(CallbackRejection::DuplicateFinal);
                }
                let Some(request) = self.requests.get_mut(&request_id) else {
                    return CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest);
                };
                request.pending_final = Some(PendingFinal {
                    callback_id: callback.callback_id,
                    text,
                    received_at_ms: callback.occurred_at_ms,
                    provider_session_id: callback.provider_session_id,
                    provider_turn_id: callback.provider_turn_id,
                });
                if settled_after_submission {
                    match self.complete_pending_final(callback.agent_id, callback.occurred_at_ms) {
                        Ok(()) => CallbackDisposition::AcceptedCompleted,
                        Err(_) => CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest),
                    }
                } else {
                    CallbackDisposition::AcceptedPendingSettlement
                }
            }
            CallbackEventKind::Error { message } => {
                if let Some(agent) = self.agents.get_mut(&callback.agent_id) {
                    agent.actionable_error = Some(message);
                }
                CallbackDisposition::AcceptedError
            }
        }
    }

    pub(crate) fn next_queued_request(&self, agent: AgentId) -> Option<RequestId> {
        let agent_state = self.agents.get(&agent)?;
        if agent_state.current_request.is_some() || agent_state.deletion_pending {
            return None;
        }
        self.queues.get(&agent)?.first().copied()
    }

    pub(crate) fn queued_requests(&self, agent: AgentId) -> &[RequestId] {
        self.queues.get(&agent).map(Vec::as_slice).unwrap_or(&[])
    }

    pub(crate) fn select_room(&mut self, room: RoomId) -> Result<(), ModelError> {
        self.mark_room_seen(room)?;
        self.visible_room = Some(room);
        Ok(())
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

    fn complete_pending_final(
        &mut self,
        agent_id: AgentId,
        completed_at_ms: u64,
    ) -> Result<(), ModelError> {
        let request_id = self
            .agents
            .get(&agent_id)
            .ok_or(ModelError::UnknownAgent(agent_id))?
            .current_request;
        let Some(request_id) = request_id else {
            return Ok(());
        };
        let request = self
            .requests
            .get(&request_id)
            .ok_or(ModelError::UnknownRequest(request_id))?;
        let Some(pending) = request.pending_final.clone() else {
            return Ok(());
        };
        let room_id = request.room_id;
        let turn_key = provider_turn_key(
            request.expected_launch_id.as_deref().unwrap_or_default(),
            pending.provider_session_id.as_deref(),
            pending.provider_turn_id.as_deref(),
        );
        let reply = Reply {
            request_id,
            agent_id,
            text: pending.text,
            received_at_ms: pending.received_at_ms,
        };
        let room = self
            .rooms
            .get_mut(&room_id)
            .ok_or(ModelError::UnknownRoom(room_id))?;
        room.latest_replies.insert(agent_id, reply);
        if self.visible_room != Some(room_id) {
            room.unread_count = room.unread_count.saturating_add(1);
        }
        let request = self
            .requests
            .get_mut(&request_id)
            .ok_or(ModelError::UnknownRequest(request_id))?;
        request.phase = RequestPhase::Completed;
        request.completed_at_ms = Some(completed_at_ms);
        if let Some(key) = turn_key {
            self.consumed_provider_turns.insert(key);
        }
        let agent = self
            .agents
            .get_mut(&agent_id)
            .ok_or(ModelError::UnknownAgent(agent_id))?;
        agent.current_request = None;
        agent.actionable_error = None;
        Ok(())
    }
}

fn normalized_name(name: &str) -> Result<String, ModelError> {
    let name = name.trim();
    if name.is_empty() {
        Err(ModelError::InvalidName)
    } else {
        Ok(name.to_owned())
    }
}

fn provider_turn_key(
    launch_id: &str,
    session_id: Option<&str>,
    turn_id: Option<&str>,
) -> Option<String> {
    Some(format!("{launch_id}\u{0}{}\u{0}{}", session_id?, turn_id?))
}

impl Default for BusState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn serialized_agent_color(state: &BusState, id: AgentId) -> [u8; 3] {
        let document = serde_json::to_value(state).expect("serialize state");
        serde_json::from_value(document["agents"][id.0.to_string()]["color"].clone())
            .expect("each agent has a persisted RGB color")
    }

    #[test]
    fn agent_color_assignment_maximizes_room_separation_and_reserves_you() {
        let mut state = BusState::new();
        let room = state.create_room("colors").unwrap();
        // Independent reference calculation over the readable 17-step sRGB grid,
        // using Euclidean Oklab distance with [102, 255, 102] already occupied.
        for expected in [
            [221, 0, 255],
            [170, 119, 51],
            [255, 204, 255],
            [0, 136, 255],
        ] {
            let id = state
                .create_agent(room, "agent", Provider::Codex, "/repo".into(), None)
                .unwrap();
            assert_eq!(serialized_agent_color(&state, id), expected);
        }
        let another_room = state.create_room("independent room").unwrap();
        let id = state
            .create_agent(
                another_room,
                "reviewer",
                Provider::ClaudeCode,
                "/repo".into(),
                None,
            )
            .unwrap();
        assert_eq!(serialized_agent_color(&state, id), [221, 0, 255]);
    }

    #[test]
    fn agent_colors_stay_readable_and_distinct_for_many_agents() {
        // WCAG luminance, independently evaluated at the persisted RGB boundary.
        let luminance = |rgb: [u8; 3]| {
            rgb.into_iter()
                .zip([0.2126, 0.7152, 0.0722])
                .map(|(channel, weight)| {
                    let channel = f64::from(channel) / 255.0;
                    weight
                        * if channel <= 0.04045 {
                            channel / 12.92
                        } else {
                            ((channel + 0.055) / 1.055).powf(2.4)
                        }
                })
                .sum::<f64>()
        };
        let mut state = BusState::new();
        let room = state.create_room("many agents").unwrap();
        let mut occupied = BTreeSet::from([[102, 255, 102]]);
        for _ in 0..32 {
            let id = state
                .create_agent(room, "agent", Provider::Codex, "/repo".into(), None)
                .unwrap();
            let color = serialized_agent_color(&state, id);
            assert!(occupied.insert(color), "color was reused: {color:?}");
            assert!(
                !(u16::from(color[1]) > u16::from(color[0]) + 32
                    && u16::from(color[1]) > u16::from(color[2]) + 32),
                "recognizably green identity belongs to You: {color:?}"
            );
            let contrast = (luminance(color) + 0.05) / (luminance([24, 24, 28]) + 0.05);
            assert!(contrast >= 4.5, "unreadable color {color:?}: {contrast}");
        }
    }

    #[test]
    fn agent_colors_survive_rename_neighbor_deletion_and_restart() {
        let (mut state, _, first, second) = state_with_room_and_agents();
        let original = serialized_agent_color(&state, second);
        state.rename_agent(second, "new name").unwrap();
        state.delete_agent(first).unwrap();
        let reloaded: BusState =
            serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
        assert_eq!(serialized_agent_color(&reloaded, second), original);
        assert_eq!(reloaded, state);
    }

    #[test]
    fn legacy_agent_colors_backfill_without_recoloring_saved_neighbors() {
        let (state, _, first, second) = state_with_room_and_agents();
        let mut document = serde_json::to_value(&state).unwrap();
        document["agents"][first.0.to_string()]
            .as_object_mut()
            .unwrap()
            .remove("color");
        // A saved custom color must be retained and considered during migration.
        document["agents"][second.0.to_string()]["color"] = serde_json::json!([0, 136, 255]);
        let migrated: BusState = serde_json::from_value(document.clone()).unwrap();
        assert_eq!(serialized_agent_color(&migrated, second), [0, 136, 255]);
        let first_color = serialized_agent_color(&migrated, first);
        assert_ne!(first_color, [0, 136, 255]);
        assert_ne!(first_color, [102, 255, 102]);
        assert_ne!(first_color, [0, 0, 0]);
        assert_eq!(
            migrated,
            serde_json::from_value::<BusState>(document).unwrap(),
            "legacy assignment is deterministic"
        );
        assert_eq!(
            migrated,
            serde_json::from_value::<BusState>(serde_json::to_value(&migrated).unwrap()).unwrap(),
            "migration is idempotent"
        );
    }

    #[test]
    fn agent_colors_migrate_unreadable_or_reserved_saved_rgb() {
        let (mut state, _, first, second) = state_with_room_and_agents();
        state.delete_agent(second).unwrap();
        for invalid in [[0, 0, 0], [102, 255, 102], [0, 170, 0]] {
            let mut document = serde_json::to_value(&state).unwrap();
            document["agents"][first.0.to_string()]["color"] = serde_json::json!(invalid);
            let migrated: BusState = serde_json::from_value(document).unwrap();
            assert_eq!(serialized_agent_color(&migrated, first), [221, 0, 255]);
        }
    }

    fn serialized_accessible_color(state: &BusState, id: AgentId) -> [u8; 3] {
        let document = serde_json::to_value(state).expect("serialize state");
        serde_json::from_value(document["agents"][id.0.to_string()]["accessible_color"].clone())
            .expect("each agent has a persisted color blind mode RGB color")
    }

    #[test]
    fn accessible_agent_colors_persist_beside_standard_colors() {
        let (mut state, _, first, second) = state_with_room_and_agents();
        let standard = serialized_agent_color(&state, second);
        let accessible = serialized_accessible_color(&state, second);
        assert!(crate::bus::colors::is_accessible_agent_color(accessible));
        assert_ne!(serialized_accessible_color(&state, first), accessible);
        state.rename_agent(second, "new name").unwrap();
        state.delete_agent(first).unwrap();
        let reloaded: BusState =
            serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
        assert_eq!(serialized_agent_color(&reloaded, second), standard);
        assert_eq!(serialized_accessible_color(&reloaded, second), accessible);
        assert_eq!(reloaded, state);
    }

    #[test]
    fn legacy_agents_backfill_accessible_colors_in_creation_order() {
        let (state, _, first, second) = state_with_room_and_agents();
        let mut document = serde_json::to_value(&state).unwrap();
        for agent in [first, second] {
            document["agents"][agent.0.to_string()]
                .as_object_mut()
                .unwrap()
                .remove("accessible_color");
        }
        let migrated: BusState = serde_json::from_value(document).unwrap();
        assert_eq!(migrated, state, "standard colors are untouched");
    }

    fn state_with_room_and_agents() -> (BusState, RoomId, AgentId, AgentId) {
        let mut state = BusState::new();
        let room = state.create_room("launch").expect("room");
        let codex = state
            .create_agent(
                room,
                "builder",
                Provider::Codex,
                PathBuf::from("/repo"),
                Some("main".into()),
            )
            .expect("agent");
        let claude = state
            .create_agent(
                room,
                "reviewer",
                Provider::ClaudeCode,
                PathBuf::from("/repo"),
                Some("topic".into()),
            )
            .expect("agent");
        state
            .set_agent_runtime_identity(
                codex,
                AgentRuntimeIdentity {
                    launch_id: Some("launch-codex".into()),
                    terminal_id: Some("terminal-1".into()),
                    pane_id: Some("pane-1".into()),
                    session_id: Some("herdr-1".into()),
                },
            )
            .expect("identity");
        state
            .set_agent_runtime_identity(
                claude,
                AgentRuntimeIdentity {
                    launch_id: Some("launch-claude".into()),
                    terminal_id: None,
                    pane_id: None,
                    session_id: None,
                },
            )
            .expect("identity");
        (state, room, codex, claude)
    }

    #[test]
    fn deletion_cleans_owned_state_preserves_neighbors_and_never_reuses_ids() {
        let (mut state, room, agent, other) = state_with_room_and_agents();
        let request = submit_text(&mut state, room, agent, "delete active work");
        start_request(&mut state, request, "launch-codex", 10);
        let queued = submit_text(&mut state, room, agent, "delete queue");
        state.set_draft_recipients(room, [agent, other]).unwrap();
        state.rooms.get_mut(&room).unwrap().latest_replies.insert(
            agent,
            Reply {
                request_id: request,
                agent_id: agent,
                text: "old reply".into(),
                received_at_ms: 1,
            },
        );
        let unrelated_room = state.create_room("unrelated").unwrap();
        let unaffected = state.agent(other).unwrap().clone();
        state.delete_agent(agent).unwrap();
        assert_eq!(state.agent(other), Some(&unaffected));
        assert!(state.request(request).is_none());
        assert!(state.request(queued).is_none());
        assert!(state.queued_requests(agent).is_empty());
        assert_eq!(
            state.room(room).unwrap().draft.recipient_ids,
            BTreeSet::from([other])
        );
        assert!(state.room(room).unwrap().latest_replies.is_empty());
        assert!(matches!(
            state.accept_callback(ProviderCallback::final_event(
                "late",
                99,
                agent,
                "launch-codex",
                "provider-session",
                "turn-1",
                "delete active work",
                "late result"
            )),
            CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest)
        ));
        state.select_room(room).unwrap();
        state.delete_room(room).unwrap();
        assert!(state.agent(other).is_none());
        assert_eq!(state.visible_room, None);
        assert!(state.room(unrelated_room).is_some());
        let new_room = state.create_room("new").unwrap();
        assert!(new_room.0 > unrelated_room.0);
        assert!(!state.is_pristine());
        state.delete_room(new_room).unwrap();
        state.delete_room(unrelated_room).unwrap();
        assert!(!state.is_pristine());
    }

    #[test]
    fn pending_deletion_rejects_new_work_even_with_an_empty_draft() {
        let (mut state, room, agent, _) = state_with_room_and_agents();
        let request = submit_text(&mut state, room, agent, "queued");
        state.prepare_delete_room(room).unwrap();
        assert_eq!(
            state.submit_draft(room, 20),
            Err(ModelError::DeletionPending)
        );
        assert_eq!(
            state.begin_submission(request, "launch-codex", 10),
            Err(ModelError::DeletionPending)
        );
        assert_eq!(state.next_queued_request(agent), None);
        assert_eq!(
            state.create_agent(room, "late", Provider::Codex, PathBuf::from("/repo"), None),
            Err(ModelError::DeletionPending)
        );
    }

    #[test]
    fn older_saved_state_defaults_deletion_flags_to_false() {
        let (state, room, agent, _) = state_with_room_and_agents();
        let mut saved = serde_json::to_value(&state).unwrap();
        for collection in ["rooms", "agents"] {
            for value in saved[collection].as_object_mut().unwrap().values_mut() {
                value.as_object_mut().unwrap().remove("deletion_pending");
            }
        }
        let recovered: BusState = serde_json::from_value(saved).unwrap();
        assert!(!recovered.room(room).unwrap().deletion_pending);
        assert!(!recovered.agent(agent).unwrap().deletion_pending);
        assert_eq!(recovered, state);
    }

    fn submit_text(state: &mut BusState, room: RoomId, agent: AgentId, text: &str) -> RequestId {
        state.set_draft_text(room, text).expect("draft text");
        state
            .set_draft_recipients(room, [agent])
            .expect("recipients");
        state.submit_draft(room, 10).expect("submit")[0]
    }

    fn start_request(state: &mut BusState, request: RequestId, launch_id: &str, boundary: u64) {
        state
            .begin_submission(request, launch_id, boundary)
            .expect("begin submission");
        state
            .record_submission(
                request,
                SubmissionOutcome::Confirmed {
                    provider_session_id: Some("provider-session".into()),
                    provider_turn_id: Some("turn-1".into()),
                },
            )
            .expect("record submission");
        let request_state = state.request(request).expect("request");
        let agent_id = request_state.agent_id;
        let prompt_payload = request_state.prompt.rendered_payload();
        assert_eq!(
            state.accept_callback(ProviderCallback {
                callback_id: format!("trusted-start-{}", request.0),
                sequence: boundary + 1,
                occurred_at_ms: boundary + 1,
                agent_id,
                launch_id: launch_id.into(),
                provider_session_id: Some("provider-session".into()),
                provider_turn_id: Some("turn-1".into()),
                provider_prompt_id: None,
                prompt_payload: Some(prompt_payload),
                kind: CallbackEventKind::PromptStarted,
            }),
            CallbackDisposition::AcceptedBinding
        );
    }

    #[test]
    fn confirmed_provider_ids_cannot_bypass_trusted_prompt_start_binding() {
        let (mut state, room, agent, _) = state_with_room_and_agents();
        let request = submit_text(&mut state, room, agent, "same prompt");
        state
            .begin_submission(request, "launch-codex", 10)
            .expect("begin");
        state
            .record_submission(
                request,
                SubmissionOutcome::Confirmed {
                    provider_session_id: Some("provider-session".into()),
                    provider_turn_id: Some("turn-1".into()),
                },
            )
            .expect("confirmed");

        assert_eq!(
            state.accept_callback(ProviderCallback::final_event(
                "premature-final",
                11,
                agent,
                "launch-codex",
                "provider-session",
                "turn-1",
                "same prompt",
                "must not publish",
            )),
            CallbackDisposition::Rejected(CallbackRejection::UnboundFinal)
        );
        assert!(state.room(room).expect("room").latest_replies.is_empty());

        assert_eq!(
            state.accept_callback(ProviderCallback {
                callback_id: "trusted-start".into(),
                sequence: 12,
                occurred_at_ms: 12,
                agent_id: agent,
                launch_id: "launch-codex".into(),
                provider_session_id: Some("provider-session".into()),
                provider_turn_id: Some("turn-1".into()),
                provider_prompt_id: None,
                prompt_payload: Some("same prompt".into()),
                kind: CallbackEventKind::PromptStarted,
            }),
            CallbackDisposition::AcceptedBinding
        );
        assert_eq!(
            state.accept_callback(ProviderCallback::final_event(
                "bound-final",
                13,
                agent,
                "launch-codex",
                "provider-session",
                "turn-1",
                "same prompt",
                "publish this",
            )),
            CallbackDisposition::AcceptedPendingSettlement
        );
    }

    #[test]
    fn callback_prompt_matching_normalizes_terminal_line_endings_and_releases_queue() {
        let (mut state, room, agent, _) = state_with_room_and_agents();
        let request = submit_text(
            &mut state,
            room,
            agent,
            "first line\r  second line\r\nthird line",
        );
        let queued = submit_text(&mut state, room, agent, "queued next");
        state
            .begin_submission(request, "launch-codex", 10)
            .expect("begin");
        state
            .record_submission(
                request,
                SubmissionOutcome::Confirmed {
                    provider_session_id: Some("provider-session".into()),
                    provider_turn_id: Some("turn-1".into()),
                },
            )
            .expect("confirmed");

        assert_eq!(
            state.accept_callback(ProviderCallback {
                callback_id: "normalized-start".into(),
                sequence: 11,
                occurred_at_ms: 11,
                agent_id: agent,
                launch_id: "launch-codex".into(),
                provider_session_id: Some("provider-session".into()),
                provider_turn_id: Some("turn-1".into()),
                provider_prompt_id: None,
                prompt_payload: Some("first line\n  second line\nthird line".into()),
                kind: CallbackEventKind::PromptStarted,
            }),
            CallbackDisposition::AcceptedBinding
        );
        assert_eq!(
            state.accept_callback(ProviderCallback::final_event(
                "normalized-final",
                12,
                agent,
                "launch-codex",
                "provider-session",
                "turn-1",
                "first line\n  second line\nthird line",
                "finished",
            )),
            CallbackDisposition::AcceptedPendingSettlement
        );

        state
            .observe_status(agent, RuntimeStatus::Idle, 13)
            .expect("settled");

        assert_eq!(
            state.request(request).expect("request").phase,
            RequestPhase::Completed
        );
        assert_eq!(state.next_queued_request(agent), Some(queued));
        assert_eq!(
            state.room(room).expect("room").latest_replies[&agent].text,
            "finished"
        );
    }

    #[test]
    fn callback_prompt_matching_rejects_prefilled_composer_text() {
        let (mut state, room, agent, _) = state_with_room_and_agents();
        let request = submit_text(&mut state, room, agent, "hello?");
        state
            .begin_submission(request, "launch-codex", 20)
            .expect("begin");
        state
            .record_submission(
                request,
                SubmissionOutcome::Confirmed {
                    provider_session_id: Some("provider-session".into()),
                    provider_turn_id: Some("turn-1".into()),
                },
            )
            .expect("confirmed");

        assert_eq!(
            state.accept_callback(ProviderCallback {
                callback_id: "prefilled-start".into(),
                sequence: 21,
                occurred_at_ms: 21,
                agent_id: agent,
                launch_id: "launch-codex".into(),
                provider_session_id: Some("provider-session".into()),
                provider_turn_id: Some("turn-1".into()),
                provider_prompt_id: None,
                prompt_payload: Some("draft already present\nhello?".into()),
                kind: CallbackEventKind::PromptStarted,
            }),
            CallbackDisposition::Rejected(CallbackRejection::WrongPrompt)
        );
        assert!(!state.request(request).expect("request").trusted_start_bound);
    }

    #[test]
    fn room_notes_drafts_and_names_are_room_local_and_ids_survive_renames() {
        let (mut state, first, agent, _) = state_with_room_and_agents();
        let second = state.create_room("second").expect("second room");
        state.rename_room(first, "renamed").expect("rename room");
        state.rename_agent(agent, "new name").expect("rename agent");
        state
            .set_room_notes(first, "Goal\nNon-goals")
            .expect("notes");
        state.set_draft_text(first, "first draft").expect("draft");
        state.set_draft_text(second, "second draft").expect("draft");
        state
            .set_agent_details_disclosed(agent, true)
            .expect("disclosure");

        assert_eq!(state.room(first).expect("first").id, first);
        assert_eq!(state.room(first).expect("first").name, "renamed");
        assert_eq!(state.agent(agent).expect("agent").id, agent);
        assert_eq!(state.agent(agent).expect("agent").name, "new name");
        assert_eq!(state.room(first).expect("first").notes, "Goal\nNon-goals");
        assert_eq!(state.room(first).expect("first").draft.text, "first draft");
        assert_eq!(
            state.room(second).expect("second").draft.text,
            "second draft"
        );
        assert!(state.agent(agent).expect("agent").details_disclosed);
    }

    #[test]
    fn quote_escapes_quotes_and_backslashes_preserves_newlines_and_routing() {
        let (mut state, room, codex, claude) = state_with_room_and_agents();
        state
            .set_draft_text(room, "existing\n")
            .expect("existing draft");
        state
            .set_draft_recipients(room, [claude])
            .expect("recipient");
        state
            .quote_reply(room, codex, "line 1 with @reviewer\n\"quoted\" \\ path")
            .expect("quote");

        let draft = &state.room(room).expect("room").draft;
        assert_eq!(
            draft.text,
            "existing\nbuilder: \"line 1 with @reviewer\n\\\"quoted\\\" \\\\ path\"\n"
        );
        assert_eq!(
            draft.recipient_ids.iter().copied().collect::<Vec<_>>(),
            [claude]
        );
    }

    #[test]
    fn file_only_submission_builds_stable_per_agent_requests_and_clears_only_draft() {
        let (mut state, room, codex, claude) = state_with_room_and_agents();
        let first_path = PathBuf::from("/one/report.md");
        let second_path = PathBuf::from("/two/report.md");
        state.attach_file(room, first_path.clone()).expect("attach");
        state
            .attach_file(room, second_path.clone())
            .expect("attach");
        state
            .attach_file(room, first_path.clone())
            .expect("deduplicate");
        state.remove_file(room, &second_path).expect("remove");
        state
            .attach_file(room, second_path.clone())
            .expect("reattach");
        state
            .set_draft_recipients(room, [codex, claude])
            .expect("recipients");

        let requests = state.submit_draft(room, 99).expect("file-only submit");

        assert_eq!(requests.len(), 2);
        assert_ne!(requests[0], requests[1]);
        let prompt = state.request(requests[0]).expect("request").prompt.clone();
        assert_eq!(prompt.text, "");
        assert_eq!(prompt.files, vec![first_path, second_path]);
        assert_eq!(
            prompt.rendered_payload(),
            "\"/one/report.md\" \"/two/report.md\""
        );
        assert!(state.room(room).expect("room").draft.files.is_empty());
        assert_eq!(
            state
                .room(room)
                .expect("room")
                .latest_prompt
                .as_ref()
                .expect("latest")
                .files,
            prompt.files
        );
    }

    #[test]
    fn queued_requests_remain_fifo_while_active_request_is_blocked_twice() {
        let (mut state, room, agent, _) = state_with_room_and_agents();
        let first = submit_text(&mut state, room, agent, "first");
        let second = submit_text(&mut state, room, agent, "second");
        let third = submit_text(&mut state, room, agent, "third");

        assert_eq!(state.next_queued_request(agent), Some(first));
        start_request(&mut state, first, "launch-codex", 5);
        state
            .observe_status(agent, RuntimeStatus::Blocked, 20)
            .expect("blocked");
        state
            .observe_status(agent, RuntimeStatus::Working, 21)
            .expect("working");
        state
            .observe_status(agent, RuntimeStatus::Blocked, 22)
            .expect("blocked again");
        state
            .observe_status(agent, RuntimeStatus::Working, 23)
            .expect("working again");

        assert_eq!(
            state.request(first).expect("first").phase,
            RequestPhase::Active
        );
        assert_eq!(state.next_queued_request(agent), None);
        assert_eq!(state.queued_requests(agent), &[second, third]);
    }

    #[test]
    fn final_is_joined_with_settled_status_and_routes_by_stable_ids_not_visible_room_or_name() {
        let (mut state, room, agent, _) = state_with_room_and_agents();
        let other = state.create_room("other").expect("other");
        state.select_room(other).expect("select other");
        let request = submit_text(&mut state, room, agent, "build it");
        start_request(&mut state, request, "launch-codex", 8);
        state
            .rename_room(room, "renamed mid-flight")
            .expect("rename room");
        state
            .rename_agent(agent, "renamed agent")
            .expect("rename agent");

        let disposition = state.accept_callback(ProviderCallback {
            callback_id: "callback-1".into(),
            sequence: 10,
            occurred_at_ms: 25,
            agent_id: agent,
            launch_id: "launch-codex".into(),
            provider_session_id: Some("provider-session".into()),
            provider_turn_id: Some("turn-1".into()),
            provider_prompt_id: None,
            prompt_payload: Some("build it".into()),
            kind: CallbackEventKind::Final {
                text: "finished".into(),
            },
        });
        assert_eq!(disposition, CallbackDisposition::AcceptedPendingSettlement);
        assert!(state.room(room).expect("room").latest_replies.is_empty());

        state
            .observe_status(agent, RuntimeStatus::Idle, 30)
            .expect("idle");

        let reply = &state.room(room).expect("room").latest_replies[&agent];
        assert_eq!(reply.text, "finished");
        assert_eq!(reply.request_id, request);
        assert_eq!(state.room(room).expect("room").unread_count, 1);
        assert_eq!(state.room(other).expect("other").unread_count, 0);
        assert_eq!(
            state.request(request).expect("request").phase,
            RequestPhase::Completed
        );
        assert_eq!(state.next_queued_request(agent), None);
        state.select_room(room).expect("read room");
        assert_eq!(state.room(room).expect("room").unread_count, 0);
    }

    #[test]
    fn previous_reply_remains_until_correlated_final_and_visible_room_does_not_accrue_unread() {
        let (mut state, room, agent, _) = state_with_room_and_agents();
        state.select_room(room).expect("select room");
        let old = submit_text(&mut state, room, agent, "old prompt");
        start_request(&mut state, old, "launch-codex", 1);
        state.accept_callback(ProviderCallback::final_event(
            "old-final",
            3,
            agent,
            "launch-codex",
            "provider-session",
            "turn-1",
            "old prompt",
            "old reply",
        ));
        state
            .observe_status(agent, RuntimeStatus::Idle, 10)
            .expect("settled");

        let new_request = submit_text(&mut state, room, agent, "new prompt");
        state
            .begin_submission(new_request, "launch-codex", 3)
            .expect("begin");
        state
            .record_submission(
                new_request,
                SubmissionOutcome::Confirmed {
                    provider_session_id: Some("provider-session".into()),
                    provider_turn_id: Some("turn-2".into()),
                },
            )
            .expect("active");
        assert_eq!(
            state.accept_callback(ProviderCallback {
                callback_id: "new-trusted-start".into(),
                sequence: 4,
                occurred_at_ms: 11,
                agent_id: agent,
                launch_id: "launch-codex".into(),
                provider_session_id: Some("provider-session".into()),
                provider_turn_id: Some("turn-2".into()),
                provider_prompt_id: None,
                prompt_payload: Some("new prompt".into()),
                kind: CallbackEventKind::PromptStarted,
            }),
            CallbackDisposition::AcceptedBinding
        );
        state
            .observe_status(agent, RuntimeStatus::Blocked, 11)
            .expect("blocked");

        assert_eq!(
            state.room(room).expect("room").latest_replies[&agent].text,
            "old reply"
        );
        assert_eq!(state.room(room).expect("room").unread_count, 0);

        state.accept_callback(ProviderCallback::final_event(
            "new-final",
            5,
            agent,
            "launch-codex",
            "provider-session",
            "turn-2",
            "new prompt",
            "new reply",
        ));
        state
            .observe_status(agent, RuntimeStatus::Idle, 12)
            .expect("settled");
        assert_eq!(
            state.room(room).expect("room").latest_replies[&agent].text,
            "new reply"
        );
        assert_eq!(state.room(room).expect("room").unread_count, 0);
    }

    #[test]
    fn duplicate_wrong_session_and_stale_pre_submit_finals_are_rejected() {
        let (mut state, room, agent, _) = state_with_room_and_agents();
        let request = submit_text(&mut state, room, agent, "same prompt");
        start_request(&mut state, request, "launch-codex", 100);

        let stale = ProviderCallback::final_event(
            "stale",
            100,
            agent,
            "launch-codex",
            "provider-session",
            "turn-1",
            "same prompt",
            "stale",
        );
        assert_eq!(
            state.accept_callback(stale),
            CallbackDisposition::Rejected(CallbackRejection::BeforeSubmissionBoundary)
        );
        let wrong = ProviderCallback::final_event(
            "wrong",
            102,
            agent,
            "launch-codex",
            "other-session",
            "turn-1",
            "same prompt",
            "wrong",
        );
        assert_eq!(
            state.accept_callback(wrong),
            CallbackDisposition::Rejected(CallbackRejection::WrongSession)
        );
        let valid = ProviderCallback::final_event(
            "valid",
            103,
            agent,
            "launch-codex",
            "provider-session",
            "turn-1",
            "same prompt",
            "valid",
        );
        assert_eq!(
            state.accept_callback(valid.clone()),
            CallbackDisposition::AcceptedPendingSettlement
        );
        assert_eq!(
            state.accept_callback(valid),
            CallbackDisposition::Rejected(CallbackRejection::DuplicateCallback)
        );
    }

    #[test]
    fn uncertain_submission_is_not_retried_but_authoritative_callback_can_finish_it() {
        let (mut state, room, agent, _) = state_with_room_and_agents();
        let request = submit_text(&mut state, room, agent, "deploy");
        state
            .begin_submission(request, "launch-codex", 40)
            .expect("begin");
        state
            .record_submission(
                request,
                SubmissionOutcome::Uncertain {
                    message: "connection lost after write".into(),
                },
            )
            .expect("uncertain");

        assert_eq!(state.next_queued_request(agent), None);
        assert!(state.request(request).expect("request").uncertain_outcome);
        assert_eq!(
            state
                .agent(agent)
                .expect("agent")
                .actionable_error
                .as_deref(),
            Some("connection lost after write")
        );

        assert_eq!(
            state.accept_callback(ProviderCallback {
                callback_id: "start-without-payload".into(),
                sequence: 41,
                occurred_at_ms: 41,
                agent_id: agent,
                launch_id: "launch-codex".into(),
                provider_session_id: Some("wrong-session".into()),
                provider_turn_id: Some("wrong-turn".into()),
                provider_prompt_id: None,
                prompt_payload: None,
                kind: CallbackEventKind::PromptStarted,
            }),
            CallbackDisposition::Rejected(CallbackRejection::WrongPrompt)
        );

        let unbound_final = ProviderCallback::final_event(
            "unbound-final",
            42,
            agent,
            "launch-codex",
            "new-session",
            "new-turn",
            "deploy",
            "wrong",
        );
        assert_eq!(
            state.accept_callback(unbound_final),
            CallbackDisposition::Rejected(CallbackRejection::UnboundFinal)
        );
        assert_eq!(
            state.accept_callback(ProviderCallback {
                callback_id: "trusted-start".into(),
                sequence: 43,
                occurred_at_ms: 45,
                agent_id: agent,
                launch_id: "launch-codex".into(),
                provider_session_id: Some("new-session".into()),
                provider_turn_id: Some("new-turn".into()),
                provider_prompt_id: Some("prompt-9".into()),
                prompt_payload: Some("deploy".into()),
                kind: CallbackEventKind::PromptStarted,
            }),
            CallbackDisposition::AcceptedBinding
        );
        assert_eq!(
            state.accept_callback(ProviderCallback::final_event(
                "late-authoritative",
                44,
                agent,
                "launch-codex",
                "new-session",
                "new-turn",
                "deploy",
                "done",
            )),
            CallbackDisposition::AcceptedPendingSettlement
        );
        state
            .observe_status(agent, RuntimeStatus::Idle, 50)
            .expect("idle");
        assert_eq!(
            state.request(request).expect("request").phase,
            RequestPhase::Completed
        );
    }

    #[test]
    fn error_callback_is_visible_and_never_becomes_a_reply() {
        let (mut state, room, agent, _) = state_with_room_and_agents();
        let request = submit_text(&mut state, room, agent, "prompt");
        start_request(&mut state, request, "launch-codex", 1);

        let result = state.accept_callback(ProviderCallback {
            callback_id: "error-event".into(),
            sequence: 3,
            occurred_at_ms: 15,
            agent_id: agent,
            launch_id: "launch-codex".into(),
            provider_session_id: Some("provider-session".into()),
            provider_turn_id: Some("turn-1".into()),
            provider_prompt_id: None,
            prompt_payload: Some("prompt".into()),
            kind: CallbackEventKind::Error {
                message: "provider session aborted".into(),
            },
        });

        assert_eq!(result, CallbackDisposition::AcceptedError);
        assert!(state.room(room).expect("room").latest_replies.is_empty());
        assert_eq!(
            state
                .agent(agent)
                .expect("agent")
                .actionable_error
                .as_deref(),
            Some("provider session aborted")
        );
        assert_eq!(
            state.request(request).expect("request").phase,
            RequestPhase::Active
        );
    }

    #[test]
    fn idle_observed_before_submission_is_not_enough_to_settle_a_later_final() {
        let (mut state, room, agent, _) = state_with_room_and_agents();
        state
            .observe_status(agent, RuntimeStatus::Idle, 1)
            .expect("pre-submit idle");
        let request = submit_text(&mut state, room, agent, "prompt");
        start_request(&mut state, request, "launch-codex", 10);

        assert_eq!(
            state.accept_callback(ProviderCallback::final_event(
                "final",
                12,
                agent,
                "launch-codex",
                "provider-session",
                "turn-1",
                "prompt",
                "done",
            )),
            CallbackDisposition::AcceptedPendingSettlement
        );
        assert_eq!(
            state.request(request).expect("request").phase,
            RequestPhase::Active
        );

        state
            .observe_status(agent, RuntimeStatus::Idle, 13)
            .expect("post-final settled idle");
        assert_eq!(
            state.request(request).expect("request").phase,
            RequestPhase::Completed
        );
    }
}
