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

/// Who wrote a prompt. Saved JSON is `"human"`, `"bus"` or `{"agent":N}`.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Author {
    Human,
    Agent(AgentId),
    /// Bus itself, such as a notice that an agent waits on a dialog.
    Bus,
}

/// Unique recipients in the order the sender selected them.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct AgentRecipients(Vec<AgentId>);

impl AgentRecipients {
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &AgentId> {
        self.0.iter()
    }

    pub(crate) fn contains(&self, id: &AgentId) -> bool {
        self.0.contains(id)
    }

    pub(crate) fn insert(&mut self, id: AgentId) -> bool {
        if self.contains(&id) {
            false
        } else {
            self.0.push(id);
            true
        }
    }

    pub(crate) fn remove(&mut self, id: &AgentId) -> bool {
        let Some(index) = self.0.iter().position(|candidate| candidate == id) else {
            return false;
        };
        self.0.remove(index);
        true
    }

    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&AgentId) -> bool) {
        self.0.retain(|id| keep(id));
    }

    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }
}

impl<'de> Deserialize<'de> for AgentRecipients {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(Vec::<AgentId>::deserialize(deserializer)?
            .into_iter()
            .collect())
    }
}

impl FromIterator<AgentId> for AgentRecipients {
    fn from_iter<T: IntoIterator<Item = AgentId>>(iter: T) -> Self {
        let mut recipients = Self::default();
        for id in iter {
            recipients.insert(id);
        }
        recipients
    }
}

impl<const N: usize> From<[AgentId; N]> for AgentRecipients {
    fn from(ids: [AgentId; N]) -> Self {
        ids.into_iter().collect()
    }
}

impl<'a> IntoIterator for &'a AgentRecipients {
    type Item = &'a AgentId;
    type IntoIter = std::slice::Iter<'a, AgentId>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl IntoIterator for AgentRecipients {
    type Item = AgentId;
    type IntoIter = std::vec::IntoIter<AgentId>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

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
    Abandoned,
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
    pub(crate) recipient_ids: AgentRecipients,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Prompt {
    pub(crate) id: PromptId,
    #[serde(default = "human_author")]
    pub(crate) author: Author,
    pub(crate) text: String,
    pub(crate) files: Vec<PathBuf>,
    pub(crate) recipient_ids: AgentRecipients,
    pub(crate) submitted_at_ms: u64,
}

fn human_author() -> Author {
    Author::Human
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
        payload_matches(payload, &self.rendered_payload())
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
    /// Bus notices for the Human, such as an agent waiting on a dialog in a
    /// room without an orchestrator. They have no recipients.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) notices: Vec<Prompt>,
    pub(crate) latest_replies: BTreeMap<AgentId, Reply>,
    #[serde(default)]
    pub(crate) deletion_pending: bool,
    #[serde(default)]
    pub(crate) kind: RoomKind,
    /// The Human's per-room sound notification choice. Unset rooms follow their
    /// kind: MASTER rings, work rooms stay silent. Read it with `sound_enabled`.
    #[serde(default)]
    pub(crate) sound: Option<bool>,
    /// The system sound this room rings with, by name; None is Bus's own ding.
    #[serde(default)]
    pub(crate) sound_name: Option<String>,
}

impl Room {
    pub(crate) fn sound_enabled(&self) -> bool {
        self.sound.unwrap_or(self.kind == RoomKind::Master)
    }
}

/// A session's rooms are units of work plus exactly one MASTER room, where the
/// orchestrator agents of those rooms live and talk to the Human.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RoomKind {
    #[default]
    Work,
    Master,
}

pub(crate) const MASTER_ROOM_NAME: &str = "MASTER";
/// Never produced by the ID allocator, which starts at 1, so adding MASTER to a
/// saved session neither collides with nor renumbers anything.
const MASTER_ROOM_ID: RoomId = RoomId(0);

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
    /// A numbered choice dialog is visible; answer it with `agent choose`.
    #[serde(default)]
    pub(crate) dialog: bool,
    /// What Bus last reported this agent waiting on: a dialog `id`, or
    /// `blocked` for a blocked screen without a readable dialog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) dialog_notice: Option<String>,
    /// The reported dialog was answered through Bus, so its closing is expected.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) dialog_answered: bool,
    /// 通过 Bus 选定的选项号，关闭通知要写明是哪一项。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) dialog_answer: Option<u32>,
    pub(crate) actionable_error: Option<String>,
    pub(crate) current_request: Option<RequestId>,
    #[serde(default)]
    pub(crate) hook_setup_confirmed: bool,
    #[serde(default)]
    pub(crate) session_binding_invalidated: bool,
    #[serde(default)]
    pub(crate) deletion_pending: bool,
    /// Set only on MASTER agents: the work room this agent orchestrates.
    #[serde(default)]
    pub(crate) orchestrates: Option<RoomId>,
    #[serde(default)]
    pub(crate) compactions: Compactions,
    /// `agent clear` reset the provider context: the next callback that names a
    /// different provider session rebinds this agent to it instead of being rejected.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) session_reset_pending: bool,
}

/// Context compactions reported by the provider's SessionStart hook
/// (`source: "compact"`). Cursor sends no such hook, so it stays at zero.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Compactions {
    pub(crate) count: u32,
    pub(crate) last_at_ms: Option<u64>,
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
    /// Sent with `--queue`: waits for an idle agent and gets its own turn.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) queue_only: bool,
    /// The lead request of the group this one joined, by being typed into the
    /// lead's running turn or coalesced into its prompt. The group shares the
    /// lead's final reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) group: Option<RequestId>,
    /// Typed into a running turn; its submit hook may arrive or not.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) steered: bool,
    /// The exact text Bus typed for a coalesced group, when it differs from
    /// this prompt alone. Submit hooks are matched against it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) submitted_payload: Option<String>,
    /// When a turn the agent ran on its own finished while this request, typed
    /// but never seen starting, waited. The agent has moved on since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) foreign_turn_settled_at_ms: Option<u64>,
}

impl Request {
    /// A Bus-authored request is a dialog notice for a room's orchestrator. It is
    /// delivered to that agent like any message but is noise for the Human, who
    /// watches the agent's terminal instead: it never shows in room history,
    /// previews, unread counts or rings.
    pub(crate) fn delivery_only(&self) -> bool {
        self.prompt.author == Author::Bus
    }

    pub(crate) fn matches_callback_payload(&self, payload: &str) -> bool {
        match &self.submitted_payload {
            Some(typed) => payload_matches(payload, typed),
            None => self.prompt.matches_callback_payload(payload),
        }
    }

    fn settled(&self) -> bool {
        matches!(
            self.phase,
            RequestPhase::Completed | RequestPhase::Abandoned
        )
    }
}

/// How long a steered group waits after its final reply for the submit hook
/// of typed input that the provider may still run as a turn of its own.
pub(crate) const STEERING_SETTLE_MS: u64 = 5_000;

/// How long an agent stays idle after finishing a turn of its own before Bus
/// gives up on a request it typed but never saw start. A queued prompt starts
/// within milliseconds of the turn before it ending.
pub(crate) const UNBOUND_SETTLE_MS: u64 = 5_000;

/// Local wall-clock `HH:MM` of a millisecond timestamp, for coalesced prompts.
fn clock(at_ms: u64) -> String {
    i64::try_from(at_ms / 1000)
        .ok()
        .and_then(crate::platform::local_datetime_at)
        .map(|local| format!("{:02}:{:02}", local.hour(), local.minute()))
        .unwrap_or_else(|| "--:--".into())
}

/// Whether a provider's submit hook reports the prompt Bus typed as `typed`.
///
/// Claude Code (2.1.291) turns each typed line that is one quoted image path
/// into an image attachment: its hook reports one `[Image #N]` placeholder per
/// image first (N counts the session's images), then the remaining lines with
/// blank ones dropped. Only that exact shape matches besides the typed text.
fn payload_matches(payload: &str, typed: &str) -> bool {
    let payload = normalized_payload(payload);
    let typed = normalized_payload(typed);
    if payload == typed {
        return true;
    }
    let (length, images) = crate::bus::callbacks::claude_image_placeholders(&payload);
    let mut lifted = 0;
    let remaining = typed
        .split('\n')
        .filter(|line| {
            let image = is_quoted_image_path(line);
            lifted += usize::from(image);
            !image && !line.trim().is_empty()
        })
        .collect::<Vec<_>>()
        .join("\n");
    images > 0 && images == lifted && payload[length..] == remaining
}

/// Whether `line` is exactly one path quoted the way `Prompt::rendered_payload`
/// quotes attachments, naming an image Claude Code attaches.
fn is_quoted_image_path(line: &str) -> bool {
    let Some(path) = line
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    else {
        return false;
    };
    let mut escaped = false;
    for character in path.chars() {
        match (escaped, character) {
            (false, '\\') => escaped = true,
            (false, '"') => return false,
            _ => escaped = false,
        }
    }
    std::path::Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["png", "jpg", "jpeg", "gif", "webp"]
                .iter()
                .any(|image| extension.eq_ignore_ascii_case(image))
        })
}

fn normalized_payload(value: &str) -> String {
    value
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim_end_matches(|character: char| character.is_ascii_whitespace())
        .to_owned()
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
    BackgroundPending,
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
    /// The callback belongs to a provider turn Bus did not start.
    UnrelatedTurn,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CallbackDisposition {
    AcceptedBinding,
    AcceptedContinuation,
    /// Input Bus typed into the running turn reached the provider.
    AcceptedSteering,
    AcceptedProgress,
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
    AgentNotIdle,
    MasterRoomFixed,
    MasterRoomHasNoNotes,
    ReservedRoomName,
    OrchestratorOutsideMaster(AgentId),
    NotOrchestratable(RoomId),
    RoomAlreadyOrchestrated { room: RoomId, agent: AgentId },
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MasterRoomFixed => {
                write!(formatter, "The MASTER room cannot be renamed or deleted")
            }
            Self::MasterRoomHasNoNotes => write!(formatter, "The MASTER room has no notes"),
            Self::ReservedRoomName => write!(
                formatter,
                "The name {MASTER_ROOM_NAME} is reserved for the master room"
            ),
            Self::OrchestratorOutsideMaster(agent) => write!(
                formatter,
                "Agent {} is not in the MASTER room; only MASTER agents orchestrate rooms",
                agent.0
            ),
            Self::NotOrchestratable(room) => write!(
                formatter,
                "Room {} cannot be orchestrated; choose an existing work room",
                room.0
            ),
            Self::RoomAlreadyOrchestrated { room, agent } => write!(
                formatter,
                "Room {} is already orchestrated by agent {}; unassign it first",
                room.0, agent.0
            ),
            Self::EmptyPrompt => write!(formatter, "The message has no text or files"),
            Self::NoRecipients => write!(formatter, "Choose at least one recipient"),
            Self::DeletionPending => write!(formatter, "That room or agent is being deleted"),
            _ => write!(formatter, "Bus model operation failed: {self:?}"),
        }
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
    /// Provider turns the agent started on its own (a task notification, or the
    /// user typing in its terminal). Their later hooks are activity, never a
    /// reply or an error for the request Bus is waiting on.
    #[serde(default)]
    unrelated_provider_turns: BTreeSet<String>,
    visible_room: Option<RoomId>,
    /// Dialog fingerprints already spent on an answer; each is single-use.
    #[serde(default)]
    consumed_dialog_fingerprints: BTreeSet<String>,
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

    fn allocate_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
    }

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
    pub(crate) fn orchestrator_of(&self, room: RoomId) -> Option<&Agent> {
        self.agents
            .values()
            .find(|agent| agent.orchestrates == Some(room))
    }

    /// Assigns (or with `None` unassigns) the work room a MASTER agent orchestrates.
    pub(crate) fn set_agent_orchestrates(
        &mut self,
        id: AgentId,
        room: Option<RoomId>,
    ) -> Result<(), ModelError> {
        let agent = self.agents.get(&id).ok_or(ModelError::UnknownAgent(id))?;
        if self
            .rooms
            .get(&agent.room_id)
            .is_none_or(|home| home.kind != RoomKind::Master)
        {
            return Err(ModelError::OrchestratorOutsideMaster(id));
        }
        if let Some(room) = room {
            if self
                .rooms
                .get(&room)
                .is_none_or(|target| target.kind != RoomKind::Work || target.deletion_pending)
            {
                return Err(ModelError::NotOrchestratable(room));
            }
            if let Some(other) = self.orchestrator_of(room).filter(|other| other.id != id) {
                return Err(ModelError::RoomAlreadyOrchestrated {
                    room,
                    agent: other.id,
                });
            }
        }
        self.agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?
            .orchestrates = room;
        Ok(())
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
                dialog: false,
                dialog_notice: None,
                dialog_answered: false,
                dialog_answer: None,
                actionable_error: None,
                current_request: None,
                hook_setup_confirmed: false,
                session_binding_invalidated: false,
                deletion_pending: false,
                orchestrates: None,
                compactions: Compactions::default(),
                session_reset_pending: false,
            },
        );
        self.queues.insert(id, Vec::new());
        Ok(id)
    }

    pub(crate) fn record_compaction(&mut self, id: AgentId, at_ms: u64) -> Result<(), ModelError> {
        let compactions = &mut self
            .agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?
            .compactions;
        compactions.count = compactions.count.saturating_add(1);
        compactions.last_at_ms = Some(at_ms);
        Ok(())
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
        let room = self.rooms.get_mut(&id).ok_or(ModelError::UnknownRoom(id))?;
        if room.kind == RoomKind::Master {
            return Err(ModelError::MasterRoomFixed);
        }
        room.deletion_pending = true;
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
        match self.rooms.get(&id) {
            None => return Err(ModelError::UnknownRoom(id)),
            Some(room) if room.kind == RoomKind::Master => return Err(ModelError::MasterRoomFixed),
            Some(_) => {}
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
        // The orchestrator stays in MASTER, unassigned.
        for agent in self.agents.values_mut() {
            if agent.orchestrates == Some(id) {
                agent.orchestrates = None;
            }
        }
        self.requests.retain(|_, request| request.room_id != id);
        // Match the UI, which falls back to the first room (MASTER) at once,
        // so `state` never shows a gap between the delete and the next view.
        if self.visible_room == Some(id) {
            self.visible_room = self.rooms.keys().next().copied();
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

    /// Marks the agent's provider context as reset by `agent clear`.
    pub(crate) fn begin_session_reset(&mut self, id: AgentId) -> Result<(), ModelError> {
        self.agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?
            .session_reset_pending = true;
        Ok(())
    }

    /// Binds a reset agent to the provider session its fresh context reported.
    /// A fresh context starts with no compactions.
    pub(crate) fn rebind_reset_session(
        &mut self,
        id: AgentId,
        session: String,
    ) -> Result<(), ModelError> {
        let agent = self
            .agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?;
        let previous = agent.runtime_identity.session_id.replace(session.clone());
        agent.session_reset_pending = false;
        agent.compactions = Compactions::default();
        // A message already sent into the fresh context was recorded under the
        // old session; its callbacks now carry the new one.
        if let Some(request) = agent
            .current_request
            .and_then(|request| self.requests.get_mut(&request))
        {
            if previous.is_some() && request.provider_session_id == previous {
                request.provider_session_id = Some(session);
            }
        }
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
        let room = self.rooms.get_mut(&id).ok_or(ModelError::UnknownRoom(id))?;
        // Notes are a work room's status board; MASTER is where the human talks to
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

    /// Queue an automation prompt without changing the room's human-owned draft.
    pub(crate) fn submit_message_from(
        &mut self,
        room: RoomId,
        draft: Draft,
        author: Author,
        now_ms: u64,
    ) -> Result<Vec<RequestId>, ModelError> {
        self.submit_message_with(room, draft, author, now_ms, false)
    }

    /// `queue_only` (`send --queue`) waits for an idle agent instead of
    /// steering a running turn, and is never coalesced with other messages.
    pub(crate) fn submit_message_with(
        &mut self,
        room: RoomId,
        draft: Draft,
        author: Author,
        now_ms: u64,
        queue_only: bool,
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
        let result = self.submit_draft_from(room, author, now_ms, queue_only);
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
        self.submit_draft_from(room, Author::Human, now_ms, false)
    }

    /// Sends the Human's draft to wait for each idle agent and its own turn.
    pub(crate) fn submit_draft_queued(
        &mut self,
        room: RoomId,
        now_ms: u64,
    ) -> Result<Vec<RequestId>, ModelError> {
        self.submit_draft_from(room, Author::Human, now_ms, true)
    }

    fn submit_draft_from(
        &mut self,
        room: RoomId,
        author: Author,
        now_ms: u64,
        queue_only: bool,
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
            author,
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
                    queue_only,
                    group: None,
                    steered: false,
                    submitted_payload: None,
                    foreign_turn_settled_at_ms: None,
                },
            );
            self.queues.entry(agent_id).or_default().push(request_id);
            request_ids.push(request_id);
        }
        let room_state = self
            .rooms
            .get_mut(&room)
            .ok_or(ModelError::UnknownRoom(room))?;
        // Bus dialog notices are delivery-only (see `Request::delivery_only`).
        if prompt.author != Author::Bus {
            // An agent's message is news for the Human, like a reply; their own is not.
            if prompt.author != Author::Human && self.visible_room != Some(room) {
                room_state.unread_count = room_state.unread_count.saturating_add(1);
            }
            room_state.latest_prompt = Some(prompt);
        }
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
        request_state.foreign_turn_settled_at_ms = None;
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
                request_state.submitted_payload = None;
                // Coalesced members go back behind their lead, in order.
                let members = self.group_members(request);
                for member in &members {
                    if let Some(member) = self.requests.get_mut(member) {
                        member.phase = RequestPhase::Queued;
                        member.group = None;
                    }
                }
                let queue = self.queues.entry(agent_id).or_default();
                queue.splice(0..0, std::iter::once(request).chain(members));
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

    pub(crate) fn observe_dialog(
        &mut self,
        agent: AgentId,
        dialog: bool,
    ) -> Result<(), ModelError> {
        self.agents
            .get_mut(&agent)
            .ok_or(ModelError::UnknownAgent(agent))?
            .dialog = dialog;
        Ok(())
    }

    /// Records what Bus reported the agent waiting on, or `None` once it cleared.
    pub(crate) fn set_dialog_notice(
        &mut self,
        agent: AgentId,
        notice: Option<String>,
    ) -> Result<(), ModelError> {
        let agent = self
            .agents
            .get_mut(&agent)
            .ok_or(ModelError::UnknownAgent(agent))?;
        agent.dialog_notice = notice;
        agent.dialog_answered = false;
        agent.dialog_answer = None;
        Ok(())
    }

    pub(crate) fn mark_dialog_answered(
        &mut self,
        agent: AgentId,
        option: u32,
    ) -> Result<(), ModelError> {
        let agent = self
            .agents
            .get_mut(&agent)
            .ok_or(ModelError::UnknownAgent(agent))?;
        agent.dialog_answered = true;
        agent.dialog_answer = Some(option);
        Ok(())
    }

    /// Posts a Bus notice for the Human in `room`; it is delivered to no agent.
    pub(crate) fn post_notice(
        &mut self,
        room: RoomId,
        text: String,
        now_ms: u64,
    ) -> Result<PromptId, ModelError> {
        const KEPT_NOTICES: usize = 50;
        let id = PromptId(self.allocate_id());
        let visible = self.visible_room == Some(room);
        let room = self
            .rooms
            .get_mut(&room)
            .ok_or(ModelError::UnknownRoom(room))?;
        room.notices.push(Prompt {
            id,
            author: Author::Bus,
            text,
            files: Vec::new(),
            recipient_ids: AgentRecipients::default(),
            submitted_at_ms: now_ms,
        });
        let excess = room.notices.len().saturating_sub(KEPT_NOTICES);
        room.notices.drain(..excess);
        if !visible {
            room.unread_count = room.unread_count.saturating_add(1);
        }
        Ok(id)
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
            let settled_idle = self.unbound_request(agent).is_some_and(|request| {
                request
                    .foreign_turn_settled_at_ms
                    .is_some_and(|at| now_ms >= at.saturating_add(UNBOUND_SETTLE_MS))
            });
            if settled_idle {
                self.release_unbound_request(agent, now_ms);
            }
        }
        Ok(())
    }

    /// The agent's current request when Bus typed it but never saw it start:
    /// typed, never bound by a submit hook, and with no reply yet.
    fn unbound_request(&self, agent: AgentId) -> Option<&Request> {
        let request = self
            .requests
            .get(&self.agents.get(&agent)?.current_request?)?;
        // An uncertain submit outcome stays Submitting until a hook binds it.
        (matches!(
            request.phase,
            RequestPhase::Active | RequestPhase::Submitting
        ) && !request.steered
            && !request.trusted_start_bound
            && request.pending_final.is_none())
        .then_some(request)
    }

    /// Releases the agent's queue from a request it never started as a turn of
    /// its own, once the agent proved it moved on: it finished a turn of its
    /// own and then started another, began a new provider session, or went
    /// idle. The typed text may have joined the agent's own turn, so its reply,
    /// if any, is in the terminal; a Bus notice in the room says so.
    pub(crate) fn release_unbound_request(
        &mut self,
        agent: AgentId,
        now_ms: u64,
    ) -> Option<RequestId> {
        let request = self.unbound_request(agent)?.id;
        let (room, prompt) = {
            let request = self.requests.get(&request)?;
            (request.room_id, request.prompt.id)
        };
        self.abandon_current_request(request, now_ms).ok()?;
        tracing::info!(
            event = "bus.message.recovered",
            request_id = request.0,
            agent_id = agent.0,
            reason = "never_started",
            "Unstarted request released"
        );
        let name = self
            .agents
            .get(&agent)
            .map(|a| a.name.clone())
            .unwrap_or_default();
        let _ = self.post_notice(
            room,
            format!(
                "{name} never started message {} as a turn of its own and has moved on, so Bus stopped waiting for its reply and delivers the next messages. Any answer to it is in {name}'s terminal.",
                prompt.0
            ),
            now_ms,
        );
        Some(request)
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
        let turn_key = provider_turn_key(
            &callback.launch_id,
            callback.provider_session_id.as_deref(),
            callback.provider_turn_id.as_deref(),
        );
        if let Some(key) = turn_key
            .as_ref()
            .filter(|key| self.unrelated_provider_turns.contains(*key))
        {
            // Text Bus typed while the agent ran a turn of its own can join
            // that turn: its submit hook then names that turn, which carries
            // the reply.
            let absorbed = matches!(callback.kind, CallbackEventKind::PromptStarted)
                && self
                    .unbound_request(callback.agent_id)
                    .is_some_and(|request| {
                        request.expected_launch_id.as_deref() == Some(callback.launch_id.as_str())
                            && request
                                .submission_boundary
                                .is_some_and(|boundary| callback.sequence > boundary)
                            && callback
                                .prompt_payload
                                .as_deref()
                                .is_some_and(|payload| request.matches_callback_payload(payload))
                    });
            if !absorbed {
                if matches!(
                    callback.kind,
                    CallbackEventKind::Final { .. } | CallbackEventKind::Error { .. }
                ) {
                    self.unrelated_provider_turns.remove(key);
                    self.note_foreign_turn_settled(&callback);
                }
                return CallbackDisposition::Rejected(CallbackRejection::UnrelatedTurn);
            }
            self.unrelated_provider_turns.remove(key);
            return self.accept_callback_for_request(callback, turn_key);
        }
        self.accept_callback_for_request(callback, turn_key)
    }

    /// Records that a turn the agent ran on its own finished after Bus typed
    /// its unbound current request.
    fn note_foreign_turn_settled(&mut self, callback: &ProviderCallback) {
        let Some(request) = self.unbound_request(callback.agent_id).map(|r| r.id) else {
            return;
        };
        if let Some(request) = self.requests.get_mut(&request).filter(|request| {
            request.expected_launch_id.as_deref() == Some(callback.launch_id.as_str())
                && request
                    .submission_boundary
                    .is_some_and(|boundary| callback.sequence > boundary)
        }) {
            request
                .foreign_turn_settled_at_ms
                .get_or_insert(callback.occurred_at_ms);
        }
    }

    fn accept_callback_for_request(
        &mut self,
        callback: ProviderCallback,
        turn_key: Option<String>,
    ) -> CallbackDisposition {
        let Some(agent) = self.agents.get(&callback.agent_id) else {
            return CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest);
        };
        let Some(request_id) = agent.current_request else {
            if matches!(callback.kind, CallbackEventKind::PromptStarted) {
                self.unrelated_provider_turns.extend(turn_key);
            }
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
        // Input Bus typed into this turn binds where the provider reports it:
        // in the running turn, or in a turn of its own that then carries the
        // group's reply. It is never an unrelated turn or an agent error.
        if let (CallbackEventKind::PromptStarted, Some(payload), true) = (
            &callback.kind,
            callback.prompt_payload.as_deref(),
            request.trusted_start_bound,
        ) {
            let steered = self.group_members(request_id).into_iter().find(|member| {
                self.requests.get(member).is_some_and(|m| {
                    m.steered && !m.settled() && m.matches_callback_payload(payload)
                })
            });
            if let Some(member) = steered {
                if let Some(member) = self.requests.get_mut(&member) {
                    member.trusted_start_bound = true;
                    member.uncertain_outcome = false;
                    member.provider_session_id = callback.provider_session_id.clone();
                    member.provider_turn_id = callback.provider_turn_id.clone();
                }
                if let Some(lead) = self.requests.get_mut(&request_id) {
                    if callback.provider_turn_id.is_some()
                        && callback.provider_turn_id != lead.provider_turn_id
                    {
                        lead.provider_turn_id = callback.provider_turn_id;
                        lead.provider_prompt_id = callback.provider_prompt_id;
                        lead.pending_final = None;
                    }
                }
                return CallbackDisposition::AcceptedSteering;
            }
        }
        // A turn that wakes after the request paused for background work carries
        // the real reply. Once the request holds a final reply, a new turn is the
        // agent's own activity and must not replace or discard that reply.
        let continuation = matches!(callback.kind, CallbackEventKind::PromptStarted)
            && request.trusted_start_bound
            && request.pending_final.is_none()
            && request.provider_session_id.is_some()
            && callback.provider_session_id == request.provider_session_id
            && request.provider_turn_id.is_some()
            && callback.provider_turn_id.is_some()
            && callback.provider_turn_id != request.provider_turn_id
            && callback
                .prompt_payload
                .as_deref()
                .is_some_and(|payload| !request.matches_callback_payload(payload));
        if matches!(callback.kind, CallbackEventKind::PromptStarted)
            && !continuation
            && callback.provider_turn_id.is_some()
            && callback.provider_turn_id != request.provider_turn_id
            && callback
                .prompt_payload
                .as_deref()
                .is_some_and(|payload| !request.matches_callback_payload(payload))
        {
            // A new turn of its own after one already finished: the typed
            // request is not coming, so the queue moves on.
            if !request.trusted_start_bound && request.foreign_turn_settled_at_ms.is_some() {
                self.release_unbound_request(callback.agent_id, callback.occurred_at_ms);
            }
            self.unrelated_provider_turns.extend(turn_key);
            return CallbackDisposition::Rejected(CallbackRejection::UnrelatedTurn);
        }
        if let Some(expected) = request.provider_turn_id.as_deref() {
            if !continuation && callback.provider_turn_id.as_deref() != Some(expected) {
                return CallbackDisposition::Rejected(CallbackRejection::WrongTurn);
            }
        }
        if let Some(expected) = request.provider_prompt_id.as_deref() {
            if !continuation
                && callback
                    .provider_prompt_id
                    .as_deref()
                    .is_some_and(|actual| actual != expected)
            {
                return CallbackDisposition::Rejected(CallbackRejection::WrongPrompt);
            }
        }
        if !continuation
            && callback
                .prompt_payload
                .as_deref()
                .is_some_and(|payload| !request.matches_callback_payload(payload))
        {
            return CallbackDisposition::Rejected(CallbackRejection::WrongPrompt);
        }

        match callback.kind {
            CallbackEventKind::PromptStarted => {
                if callback.provider_session_id.is_none() || callback.provider_turn_id.is_none() {
                    return CallbackDisposition::Rejected(CallbackRejection::WrongPrompt);
                }
                if !continuation
                    && !callback
                        .prompt_payload
                        .as_deref()
                        .is_some_and(|payload| request.matches_callback_payload(payload))
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
                request.pending_final = None;
                if let Some(agent) = self.agents.get_mut(&callback.agent_id) {
                    agent.actionable_error = None;
                }
                if continuation {
                    CallbackDisposition::AcceptedContinuation
                } else {
                    CallbackDisposition::AcceptedBinding
                }
            }
            CallbackEventKind::BackgroundPending => {
                if !request.trusted_start_bound
                    || request.provider_session_id.is_none()
                    || request.provider_turn_id.is_none()
                {
                    return CallbackDisposition::Rejected(CallbackRejection::UnboundFinal);
                }
                let Some(request) = self.requests.get_mut(&request_id) else {
                    return CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest);
                };
                request.pending_final = None;
                CallbackDisposition::AcceptedProgress
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
                        Ok(true) => CallbackDisposition::AcceptedCompleted,
                        Ok(false) => CallbackDisposition::AcceptedPendingSettlement,
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

    /// Whether the callback belongs to a turn the agent started on its own.
    pub(crate) fn is_unrelated_turn(&self, callback: &ProviderCallback) -> bool {
        provider_turn_key(
            &callback.launch_id,
            callback.provider_session_id.as_deref(),
            callback.provider_turn_id.as_deref(),
        )
        .is_some_and(|key| self.unrelated_provider_turns.contains(&key))
    }

    /// The request a message may steer into: the agent's current request while
    /// its turn visibly runs, bound by its submit hook, with no final reply yet.
    /// Returns the first queued request and that lead, unless the queued one
    /// asked to wait (`--queue`).
    pub(crate) fn next_steering(&self, agent: AgentId) -> Option<(RequestId, RequestId)> {
        let agent_state = self.agents.get(&agent)?;
        if agent_state.status != RuntimeStatus::Working
            || agent_state.dialog
            || agent_state.deletion_pending
        {
            return None;
        }
        let lead = self.requests.get(&agent_state.current_request?)?;
        if lead.phase != RequestPhase::Active
            || !lead.trusted_start_bound
            || lead.pending_final.is_some()
        {
            return None;
        }
        let first = *self.queues.get(&agent)?.first()?;
        (!self.requests.get(&first)?.queue_only).then_some((first, lead.id))
    }

    /// Moves `request` from the queue into `lead`'s group before Bus types it
    /// into the running turn. The lead then settles only on an idle status
    /// observed after this, so the turn the typing extends is not cut short.
    pub(crate) fn begin_steering(
        &mut self,
        request: RequestId,
        lead: RequestId,
    ) -> Result<(), ModelError> {
        let agent_id = self
            .requests
            .get(&request)
            .ok_or(ModelError::UnknownRequest(request))?
            .agent_id;
        let agent = self
            .agents
            .get(&agent_id)
            .ok_or(ModelError::UnknownAgent(agent_id))?;
        if agent.current_request != Some(lead)
            || self.queues.get(&agent_id).and_then(|queue| queue.first()) != Some(&request)
        {
            return Err(ModelError::InvalidTransition);
        }
        let status_revision = agent.status_revision;
        let launch = self
            .requests
            .get(&lead)
            .ok_or(ModelError::UnknownRequest(lead))?
            .expected_launch_id
            .clone();
        if let Some(queue) = self.queues.get_mut(&agent_id) {
            queue.remove(0);
        }
        let request_state = self
            .requests
            .get_mut(&request)
            .ok_or(ModelError::UnknownRequest(request))?;
        request_state.phase = RequestPhase::Submitting;
        request_state.expected_launch_id = launch;
        request_state.group = Some(lead);
        request_state.steered = true;
        if let Some(lead) = self.requests.get_mut(&lead) {
            lead.submission_status_revision = status_revision;
        }
        Ok(())
    }

    /// Records the native outcome of typing a steering message. A definite
    /// rejection (the agent stopped working) returns it to the queue front.
    pub(crate) fn record_steering(
        &mut self,
        request: RequestId,
        outcome: SubmissionOutcome,
    ) -> Result<(), ModelError> {
        let request_state = self
            .requests
            .get_mut(&request)
            .ok_or(ModelError::UnknownRequest(request))?;
        if request_state.phase != RequestPhase::Submitting || !request_state.steered {
            return Err(ModelError::InvalidTransition);
        }
        match outcome {
            SubmissionOutcome::Confirmed { .. } => request_state.phase = RequestPhase::Active,
            SubmissionOutcome::Uncertain { .. } => {
                request_state.phase = RequestPhase::Active;
                request_state.uncertain_outcome = true;
            }
            SubmissionOutcome::DefinitelyRejected { .. } => {
                request_state.phase = RequestPhase::Queued;
                request_state.expected_launch_id = None;
                request_state.group = None;
                request_state.steered = false;
                let agent_id = request_state.agent_id;
                self.queues.entry(agent_id).or_default().insert(0, request);
            }
        }
        Ok(())
    }

    /// Joins the queued messages that piled up while the agent could not take
    /// them into `lead`'s prompt: consecutive ones from the queue front, up to
    /// the first sent with `--queue`. Returns the text to type, each part
    /// marked with its sender and time, or `None` when `lead` goes alone.
    pub(crate) fn coalesce_queue(&mut self, lead: RequestId) -> Option<String> {
        let agent_id = self.requests.get(&lead)?.agent_id;
        let queue = self.queues.get(&agent_id)?;
        if queue.first() != Some(&lead) || self.requests.get(&lead)?.queue_only {
            return None;
        }
        let members: Vec<RequestId> = queue[1..]
            .iter()
            .copied()
            .take_while(|id| self.requests.get(id).is_some_and(|r| !r.queue_only))
            .collect();
        if members.is_empty() {
            return None;
        }
        let parts: Vec<RequestId> = std::iter::once(lead)
            .chain(members.iter().copied())
            .collect();
        let count = parts.len();
        let text = parts
            .iter()
            .enumerate()
            .filter_map(|(index, id)| {
                let prompt = &self.requests.get(id)?.prompt;
                Some(format!(
                    "[{}/{count} from {} at {}]\n{}",
                    index + 1,
                    self.sender_name(&prompt.author),
                    clock(prompt.submitted_at_ms),
                    prompt.rendered_payload()
                ))
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        if let Some(queue) = self.queues.get_mut(&agent_id) {
            queue.retain(|id| !members.contains(id));
        }
        for member in members {
            if let Some(request) = self.requests.get_mut(&member) {
                request.phase = RequestPhase::Active;
                request.group = Some(lead);
            }
        }
        if let Some(request) = self.requests.get_mut(&lead) {
            request.submitted_payload = Some(text.clone());
        }
        Some(text)
    }

    fn sender_name(&self, author: &Author) -> String {
        match author {
            Author::Human => "the human".into(),
            Author::Bus => "Bus".into(),
            Author::Agent(id) => self
                .agents
                .get(id)
                .map_or_else(|| "an agent".into(), |agent| agent.name.clone()),
        }
    }

    /// The requests that joined `lead`'s group, oldest first.
    pub(crate) fn group_members(&self, lead: RequestId) -> Vec<RequestId> {
        self.requests
            .values()
            .filter(|request| request.group == Some(lead))
            .map(|request| request.id)
            .collect()
    }

    pub(crate) fn next_queued_request(&self, agent: AgentId) -> Option<RequestId> {
        let agent_state = self.agents.get(&agent)?;
        if agent_state.current_request.is_some() || agent_state.deletion_pending {
            return None;
        }
        self.queues.get(&agent)?.first().copied()
    }

    pub(crate) fn recover_idle_request(
        &mut self,
        request: RequestId,
        recovered_at_ms: u64,
    ) -> Result<(), ModelError> {
        let request_state = self
            .requests
            .get(&request)
            .ok_or(ModelError::UnknownRequest(request))?;
        if !matches!(
            request_state.phase,
            RequestPhase::Submitting | RequestPhase::Active
        ) {
            return Err(ModelError::InvalidTransition);
        }
        let agent_id = request_state.agent_id;
        let agent = self
            .agents
            .get(&agent_id)
            .ok_or(ModelError::UnknownAgent(agent_id))?;
        if agent.current_request != Some(request) {
            return Err(ModelError::InvalidTransition);
        }
        if agent.status != RuntimeStatus::Idle {
            return Err(ModelError::AgentNotIdle);
        }
        self.abandon_current_request(request, recovered_at_ms)
    }

    /// Abandons `request`, its agent's current one, with its group, and frees the agent.
    fn abandon_current_request(
        &mut self,
        request: RequestId,
        recovered_at_ms: u64,
    ) -> Result<(), ModelError> {
        let request_state = self
            .requests
            .get_mut(&request)
            .ok_or(ModelError::UnknownRequest(request))?;
        let agent_id = request_state.agent_id;
        request_state.phase = RequestPhase::Abandoned;
        request_state.pending_final = None;
        request_state.completed_at_ms = Some(recovered_at_ms);
        for member in self.group_members(request) {
            if let Some(member) = self.requests.get_mut(&member).filter(|m| !m.settled()) {
                member.phase = RequestPhase::Abandoned;
                member.completed_at_ms = Some(recovered_at_ms);
            }
        }
        let agent = self
            .agents
            .get_mut(&agent_id)
            .ok_or(ModelError::UnknownAgent(agent_id))?;
        agent.current_request = None;
        agent.actionable_error = None;
        Ok(())
    }

    pub(crate) fn queued_requests(&self, agent: AgentId) -> &[RequestId] {
        self.queues.get(&agent).map(Vec::as_slice).unwrap_or(&[])
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

    /// Settles the agent's current request, and every request in its group,
    /// with its pending final reply. Returns whether it settled: a group with
    /// typed input whose submit hook has not arrived waits a few seconds, in
    /// case the provider runs that input as a turn of its own.
    fn complete_pending_final(
        &mut self,
        agent_id: AgentId,
        completed_at_ms: u64,
    ) -> Result<bool, ModelError> {
        let request_id = self
            .agents
            .get(&agent_id)
            .ok_or(ModelError::UnknownAgent(agent_id))?
            .current_request;
        let Some(request_id) = request_id else {
            return Ok(false);
        };
        let request = self
            .requests
            .get(&request_id)
            .ok_or(ModelError::UnknownRequest(request_id))?;
        let Some(pending) = request.pending_final.clone() else {
            return Ok(false);
        };
        let members = self.group_members(request_id);
        let unseen_steering = members.iter().any(|member| {
            self.requests
                .get(member)
                .is_some_and(|m| m.steered && !m.trusted_start_bound && !m.settled())
        });
        if unseen_steering
            && completed_at_ms < pending.received_at_ms.saturating_add(STEERING_SETTLE_MS)
        {
            return Ok(false);
        }
        let room_id = request.room_id;
        let delivery_only = request.delivery_only();
        let turn_key = provider_turn_key(
            request.expected_launch_id.as_deref().unwrap_or_default(),
            pending.provider_session_id.as_deref(),
            pending.provider_turn_id.as_deref(),
        );
        let reply = Reply {
            request_id,
            agent_id,
            text: pending.text.clone(),
            received_at_ms: pending.received_at_ms,
        };
        let room = self
            .rooms
            .get_mut(&room_id)
            .ok_or(ModelError::UnknownRoom(room_id))?;
        // The reply to a delivery-only dialog notice is as hidden as the notice.
        if !delivery_only {
            room.latest_replies.insert(agent_id, reply);
            if self.visible_room != Some(room_id) {
                room.unread_count = room.unread_count.saturating_add(1);
            }
        }
        for member in members {
            if let Some(member) = self.requests.get_mut(&member).filter(|m| !m.settled()) {
                member.phase = RequestPhase::Completed;
                member.completed_at_ms = Some(completed_at_ms);
                member.pending_final = Some(pending.clone());
            }
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
        Ok(true)
    }
}

fn work_room_name(name: &str) -> Result<String, ModelError> {
    let name = normalized_name(name)?;
    if name.eq_ignore_ascii_case(MASTER_ROOM_NAME) {
        return Err(ModelError::ReservedRoomName);
    }
    Ok(name)
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

    fn master_agent(state: &mut BusState, name: &str) -> AgentId {
        let master = state.ensure_master_room();
        state
            .create_agent(master, name, Provider::ClaudeCode, "/repo".into(), None)
            .expect("master agent")
    }

    #[test]
    fn master_room_is_added_once_without_consuming_ids() {
        let mut state = BusState::new();
        let master = state.ensure_master_room();
        assert_eq!(state.ensure_master_room(), master);
        let masters = state
            .rooms()
            .filter(|room| room.kind == RoomKind::Master)
            .collect::<Vec<_>>();
        assert_eq!(masters.len(), 1);
        assert_eq!(masters[0].name, MASTER_ROOM_NAME);
        // Seeding the first work room still sees a pristine session.
        assert!(state.is_pristine());
        assert!(!state.has_work());
        let work = state.create_room("work").unwrap();
        assert_eq!(work, RoomId(1));
        assert_eq!(state.rooms().next().map(|room| room.id), Some(master));
        assert!(state.has_work());
    }

    #[test]
    fn sound_rings_by_default_only_in_master_until_the_human_chooses() {
        let mut state = BusState::new();
        let master = state.ensure_master_room();
        let work = state.create_room("work").unwrap();
        assert!(state.room(master).unwrap().sound_enabled());
        assert!(!state.room(work).unwrap().sound_enabled());

        state.set_room_sound(master, false).unwrap();
        state.set_room_sound(work, true).unwrap();
        assert!(!state.room(master).unwrap().sound_enabled());
        assert!(state.room(work).unwrap().sound_enabled());
        assert_eq!(
            state.set_room_sound(RoomId(999), true),
            Err(ModelError::UnknownRoom(RoomId(999)))
        );

        // A MASTER room saved before the sound field existed still rings.
        let mut document = serde_json::to_value(&state).unwrap();
        let rooms = document["rooms"].as_object_mut().unwrap();
        rooms.get_mut(&master.0.to_string()).unwrap()["sound"] = serde_json::Value::Null;
        rooms
            .get_mut(&work.0.to_string())
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("sound");
        let loaded: BusState = serde_json::from_value(document).unwrap();
        assert!(loaded.room(master).unwrap().sound_enabled());
        assert!(!loaded.room(work).unwrap().sound_enabled());
    }

    #[test]
    fn room_sound_names_persist_and_old_sessions_load_with_the_default() {
        let mut state = BusState::new();
        let master = state.ensure_master_room();
        let work = state.create_room("work").unwrap();
        state
            .set_room_sound_name(work, Some("Glass".into()))
            .unwrap();
        assert_eq!(
            state.set_room_sound_name(RoomId(999), None),
            Err(ModelError::UnknownRoom(RoomId(999)))
        );
        let mut document = serde_json::to_value(&state).unwrap();
        let loaded: BusState = serde_json::from_value(document.clone()).unwrap();
        assert_eq!(
            loaded.room(work).unwrap().sound_name.as_deref(),
            Some("Glass")
        );
        assert_eq!(loaded.room(master).unwrap().sound_name, None);

        // Sessions saved before sound names existed play Bus's own ding.
        document["rooms"][work.0.to_string()]
            .as_object_mut()
            .unwrap()
            .remove("sound_name");
        let loaded: BusState = serde_json::from_value(document).unwrap();
        assert_eq!(loaded.room(work).unwrap().sound_name, None);
    }

    #[test]
    fn master_room_has_no_notes_and_work_rooms_keep_theirs() {
        let mut state = BusState::new();
        let master = state.ensure_master_room();
        let work = state.create_room("work").unwrap();
        assert_eq!(
            state.set_room_notes(master, "status"),
            Err(ModelError::MasterRoomHasNoNotes)
        );
        assert_eq!(state.room(master).unwrap().notes, "");
        state.set_room_notes(work, "status").unwrap();
        assert_eq!(state.room(work).unwrap().notes, "status");
    }

    #[test]
    fn master_room_cannot_be_renamed_or_deleted_and_its_name_is_reserved() {
        let mut state = BusState::new();
        let master = state.ensure_master_room();
        assert_eq!(
            state.rename_room(master, "other"),
            Err(ModelError::MasterRoomFixed)
        );
        assert_eq!(
            state.prepare_delete_room(master),
            Err(ModelError::MasterRoomFixed)
        );
        assert_eq!(state.delete_room(master), Err(ModelError::MasterRoomFixed));
        assert!(!state.room(master).unwrap().deletion_pending);
        for name in ["MASTER", "master", " Master "] {
            assert_eq!(state.create_room(name), Err(ModelError::ReservedRoomName));
        }
        let work = state.create_room("work").unwrap();
        assert_eq!(
            state.rename_room(work, "master"),
            Err(ModelError::ReservedRoomName)
        );
        assert_eq!(
            ModelError::MasterRoomFixed.to_string(),
            "The MASTER room cannot be renamed or deleted"
        );
    }

    #[test]
    fn each_work_room_has_at_most_one_master_orchestrator() {
        let mut state = BusState::new();
        let master = state.ensure_master_room();
        let pr = state.create_room("pr-123").unwrap();
        let other = state.create_room("pr-456").unwrap();
        let first = master_agent(&mut state, "claude-orch");
        let second = master_agent(&mut state, "codex-orch");

        state.set_agent_orchestrates(first, Some(pr)).unwrap();
        assert_eq!(state.orchestrator_of(pr).map(|agent| agent.id), Some(first));
        let taken = state.set_agent_orchestrates(second, Some(pr));
        assert_eq!(
            taken,
            Err(ModelError::RoomAlreadyOrchestrated {
                room: pr,
                agent: first
            })
        );
        assert!(taken
            .unwrap_err()
            .to_string()
            .contains("already orchestrated by agent"));
        // Reassigning the same agent, moving it, and unassigning all succeed.
        state.set_agent_orchestrates(first, Some(pr)).unwrap();
        state.set_agent_orchestrates(first, Some(other)).unwrap();
        state.set_agent_orchestrates(second, Some(pr)).unwrap();
        state.set_agent_orchestrates(second, None).unwrap();
        assert!(state.orchestrator_of(pr).is_none());

        assert_eq!(
            state.set_agent_orchestrates(first, Some(master)),
            Err(ModelError::NotOrchestratable(master))
        );
        assert_eq!(
            state.set_agent_orchestrates(first, Some(RoomId(999))),
            Err(ModelError::NotOrchestratable(RoomId(999)))
        );
        state.prepare_delete_room(pr).unwrap();
        assert_eq!(
            state.set_agent_orchestrates(second, Some(pr)),
            Err(ModelError::NotOrchestratable(pr))
        );
    }

    #[test]
    fn only_master_agents_orchestrate_rooms() {
        let mut state = BusState::new();
        state.ensure_master_room();
        let pr = state.create_room("pr-123").unwrap();
        let worker = state
            .create_agent(pr, "builder", Provider::Codex, "/repo".into(), None)
            .unwrap();
        assert_eq!(
            state.set_agent_orchestrates(worker, Some(pr)),
            Err(ModelError::OrchestratorOutsideMaster(worker))
        );
        assert_eq!(state.agent(worker).unwrap().orchestrates, None);
    }

    #[test]
    fn deleting_an_orchestrated_room_leaves_its_orchestrator_unassigned_in_master() {
        let mut state = BusState::new();
        let master = state.ensure_master_room();
        let pr = state.create_room("pr-123").unwrap();
        let orchestrator = master_agent(&mut state, "claude-orch");
        state
            .set_agent_orchestrates(orchestrator, Some(pr))
            .unwrap();

        state.prepare_delete_room(pr).unwrap();
        state.delete_room(pr).unwrap();

        let agent = state.agent(orchestrator).expect("orchestrator survives");
        assert_eq!(agent.room_id, master);
        assert_eq!(agent.orchestrates, None);
        assert!(!agent.deletion_pending);
    }

    #[test]
    fn ensure_master_room_drops_assignments_that_break_the_invariants() {
        let mut state = BusState::new();
        let master = state.ensure_master_room();
        let pr = state.create_room("pr-123").unwrap();
        let first = master_agent(&mut state, "first");
        let second = master_agent(&mut state, "second");
        let outsider = state
            .create_agent(pr, "builder", Provider::Codex, "/repo".into(), None)
            .unwrap();
        // Hand-edited or corrupt saved state can carry assignments the API refuses.
        state.agents.get_mut(&first).unwrap().orchestrates = Some(pr);
        state.agents.get_mut(&second).unwrap().orchestrates = Some(pr);
        state.agents.get_mut(&outsider).unwrap().orchestrates = Some(pr);
        let lost = master_agent(&mut state, "lost");
        state.agents.get_mut(&lost).unwrap().orchestrates = Some(master);

        state.ensure_master_room();

        assert_eq!(state.agent(first).unwrap().orchestrates, Some(pr));
        assert_eq!(state.agent(second).unwrap().orchestrates, None);
        assert_eq!(state.agent(outsider).unwrap().orchestrates, None);
        assert_eq!(state.agent(lost).unwrap().orchestrates, None);
    }

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
            [other].into()
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
        let fallback = state.rooms().next().map(|room| room.id);
        assert!(fallback.is_some());
        assert_eq!(state.visible_room, fallback);
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
    fn claude_long_paste_with_attached_file_binds_its_trusted_start() {
        let (mut state, room, _, claude) = state_with_room_and_agents();
        let text = "Please read the attached brief and reply for EACH of F1, F2, F3 with \
                    your recommended option and a 2-4 sentence rationale (file:line). "
            .repeat(8);
        let file = "/private/tmp/scratchpad/pr1131-review/r1-flags.md";
        state.set_draft_text(room, &text).expect("draft text");
        state
            .attach_file(room, PathBuf::from(file))
            .expect("attach");
        state
            .set_draft_recipients(room, [claude])
            .expect("recipients");
        let request = state.submit_draft(room, 10).expect("submit")[0];
        state
            .begin_submission(request, "launch-claude", 15)
            .expect("begin");
        state
            .record_submission(
                request,
                SubmissionOutcome::Confirmed {
                    provider_session_id: None,
                    provider_turn_id: None,
                },
            )
            .expect("confirmed");

        // Shape captured from Claude Code 2.1.284 for a Bus paste of text plus
        // one quoted attachment path: the whole paste is framed, path included.
        let hook = serde_json::json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": "claude-session",
            "prompt_id": "claude-prompt",
            "prompt": format!(
                "\n\n<pasted_content id=\"dff1\">\n{text}\n\"{file}\"\n</pasted_content id=\"dff1\">\n"
            ),
        });
        let crate::bus::callbacks::Parsed::Started {
            session,
            turn,
            prompt,
        } = crate::bus::callbacks::parse(Provider::ClaudeCode, &hook).expect("parse")
        else {
            panic!("UserPromptSubmit must parse as a start");
        };

        assert_eq!(
            state.accept_callback(ProviderCallback {
                callback_id: "claude-framed-start".into(),
                sequence: 16,
                occurred_at_ms: 16,
                agent_id: claude,
                launch_id: "launch-claude".into(),
                provider_session_id: Some(session),
                provider_turn_id: Some(turn.clone()),
                provider_prompt_id: Some(turn),
                prompt_payload: Some(prompt),
                kind: CallbackEventKind::PromptStarted,
            }),
            CallbackDisposition::AcceptedBinding
        );
        assert!(state.request(request).expect("request").trusted_start_bound);
    }

    #[test]
    fn claude_attached_image_binds_its_trusted_start() {
        // Shapes captured from Claude Code 2.1.291: a pasted line that is one
        // quoted image path becomes an `[Image #N]` attachment (N counts the
        // session's images), reported first, and blank lines are dropped; a
        // long remainder keeps its paste framing.
        let short = "Reply with just: pong\n\n\nsecond para  \n\nthird";
        let long = "for orchestrator agents in MASTER I just need \nso it should look like\n\n\
                    bus orchestrator Idle x\n"
            .repeat(4);
        let cases = [
            (short, "[Image #1]Reply with just: pong\nsecond para  \nthird".to_owned()),
            (
                long.as_str(),
                format!(
                    "[Image #17]\n\n<pasted_content id=\"8f5d\">\n{}\n</pasted_content id=\"8f5d\">\n",
                    long.trim_end().replace("\n\n", "\n")
                ),
            ),
            ("", "[Image #3]".to_owned()),
        ];
        for (n, (text, hook_prompt)) in cases.into_iter().enumerate() {
            let (mut state, room, _, claude) = state_with_room_and_agents();
            if !text.is_empty() {
                state.set_draft_text(room, text).expect("draft text");
            }
            state
                .attach_file(room, PathBuf::from("/tmp/bus/paste-e8d63c50.png"))
                .expect("attach");
            state
                .set_draft_recipients(room, [claude])
                .expect("recipients");
            let request = state.submit_draft(room, 10).expect("submit")[0];
            state
                .begin_submission(request, "launch-claude", 15)
                .expect("begin");
            let hook = serde_json::json!({
                "hook_event_name": "UserPromptSubmit",
                "session_id": "claude-session",
                "prompt_id": "claude-prompt",
                "prompt": hook_prompt,
            });
            let crate::bus::callbacks::Parsed::Started {
                session,
                turn,
                prompt,
            } = crate::bus::callbacks::parse(Provider::ClaudeCode, &hook).expect("parse")
            else {
                panic!("UserPromptSubmit must parse as a start");
            };
            let start = |id: &str, prompt: String| ProviderCallback {
                callback_id: id.into(),
                sequence: 16,
                occurred_at_ms: 16,
                agent_id: claude,
                launch_id: "launch-claude".into(),
                provider_session_id: Some(session.clone()),
                provider_turn_id: Some(format!("{turn}-{id}")),
                provider_prompt_id: Some(format!("{turn}-{id}")),
                prompt_payload: Some(prompt),
                kind: CallbackEventKind::PromptStarted,
            };
            // Different text, or more images than the request attached, is another turn.
            for (wrong, payload) in [
                ("text", format!("{prompt} extra")),
                ("images", format!("[Image #9]{prompt}")),
            ] {
                assert_ne!(
                    state.accept_callback(start(wrong, payload)),
                    CallbackDisposition::AcceptedBinding,
                    "case {n}: {wrong}"
                );
            }
            assert_eq!(
                state.accept_callback(start("image-start", prompt)),
                CallbackDisposition::AcceptedBinding,
                "case {n}"
            );
            assert!(state.request(request).expect("request").trusted_start_bound);
        }
    }

    #[test]
    fn claude_image_placeholders_only_stand_for_lone_image_path_lines() {
        let typed = "Look\n\n\"/tmp/a.png\"";
        assert!(payload_matches("[Image #2]Look", typed));
        assert!(payload_matches("Look\n\n\"/tmp/a.png\"", typed));
        // Two paths on one line, or a non-image file, stay as typed.
        assert!(!payload_matches(
            "[Image #1]Look",
            "Look\n\"/tmp/a.png\" \"/tmp/b.png\""
        ));
        assert!(!payload_matches("[Image #1]Look", "Look\n\"/tmp/a.diff\""));
        assert!(!payload_matches("[Image #1]", "Look\n\"/tmp/a.png\""));
        assert!(!payload_matches("[Image #x]Look", typed));
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
    fn submission_preserves_first_recipient_selection_order() {
        let (mut state, room, codex, claude) = state_with_room_and_agents();
        state
            .set_draft_recipients(room, [claude, codex, claude])
            .expect("ordered recipients");
        state.set_draft_text(room, "review this").expect("draft");

        let requests = state.submit_draft(room, 99).expect("submit");
        let request_agents = requests
            .iter()
            .map(|id| state.request(*id).expect("request").agent_id)
            .collect::<Vec<_>>();
        let prompt = &state.request(requests[0]).expect("request").prompt;

        assert_eq!(request_agents, [claude, codex]);
        assert_eq!(
            prompt.recipient_ids.iter().copied().collect::<Vec<_>>(),
            [claude, codex]
        );
        let restored: BusState =
            serde_json::from_value(serde_json::to_value(&state).expect("serialize"))
                .expect("deserialize");
        assert_eq!(
            restored
                .request(requests[0])
                .expect("restored request")
                .prompt
                .recipient_ids
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            [claude, codex]
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

    #[test]
    fn background_progress_and_same_session_continuation_wait_for_the_later_final() {
        let (mut state, room, agent, _) = state_with_room_and_agents();
        let request = submit_text(&mut state, room, agent, "review the plan");
        start_request(&mut state, request, "launch-codex", 10);

        assert_eq!(
            state.accept_callback(ProviderCallback {
                callback_id: "background-pending".into(),
                sequence: 12,
                occurred_at_ms: 12,
                agent_id: agent,
                launch_id: "launch-codex".into(),
                provider_session_id: Some("provider-session".into()),
                provider_turn_id: Some("turn-1".into()),
                provider_prompt_id: None,
                prompt_payload: None,
                kind: CallbackEventKind::BackgroundPending,
            }),
            CallbackDisposition::AcceptedProgress
        );
        state
            .observe_status(agent, RuntimeStatus::Idle, 13)
            .expect("provider paused for background work");
        assert_eq!(state.request(request).unwrap().phase, RequestPhase::Active);

        assert_eq!(
            state.accept_callback(ProviderCallback {
                callback_id: "continuation-start".into(),
                sequence: 14,
                occurred_at_ms: 14,
                agent_id: agent,
                launch_id: "launch-codex".into(),
                provider_session_id: Some("provider-session".into()),
                provider_turn_id: Some("turn-2".into()),
                provider_prompt_id: None,
                prompt_payload: Some(
                    "<task-notification>reviewer finished</task-notification>".into()
                ),
                kind: CallbackEventKind::PromptStarted,
            }),
            CallbackDisposition::AcceptedContinuation
        );
        assert_eq!(
            state.request(request).unwrap().provider_turn_id.as_deref(),
            Some("turn-2")
        );
        assert_eq!(
            state.accept_callback(ProviderCallback::final_event(
                "continuation-final",
                15,
                agent,
                "launch-codex",
                "provider-session",
                "turn-2",
                "review the plan",
                "Ready",
            )),
            CallbackDisposition::AcceptedCompleted
        );
        assert_eq!(
            state.request(request).unwrap().phase,
            RequestPhase::Completed
        );
        assert_eq!(
            state.room(room).unwrap().latest_replies[&agent].text,
            "Ready"
        );
    }

    #[test]
    fn recovery_abandons_only_an_idle_current_request_and_releases_its_agent() {
        let (mut state, room, agent, _) = state_with_room_and_agents();
        let request = submit_text(&mut state, room, agent, "wedged request");
        let queued = submit_text(&mut state, room, agent, "next request");
        start_request(&mut state, request, "launch-codex", 10);

        assert_eq!(
            state.recover_idle_request(request, 20),
            Err(ModelError::AgentNotIdle)
        );
        state
            .observe_status(agent, RuntimeStatus::Idle, 21)
            .unwrap();
        state.recover_idle_request(request, 22).unwrap();

        assert_eq!(
            state.request(request).unwrap().phase,
            RequestPhase::Abandoned
        );
        assert_eq!(state.request(request).unwrap().completed_at_ms, Some(22));
        assert_eq!(state.agent(agent).unwrap().current_request, None);
        assert_eq!(state.next_queued_request(agent), Some(queued));
        assert!(state.room(room).unwrap().latest_replies.is_empty());
    }
}
