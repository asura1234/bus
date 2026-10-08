use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};

use super::{prefs::colors, provider_glue::callbacks};

mod status;

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
    /// Agent messages to the Human (`post_to_human`); they have no recipients.
    /// Bus is not an agent and never posts here: notices older versions saved
    /// as "Bus" are dropped on load.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "deserialize_notices"
    )]
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
/// The `send --to` selector for a message to the Human; no agent may take it.
pub(crate) const HUMAN_RECIPIENT: &str = "human";
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
    /// The status revision at which the agent was last seen Working or
    /// Blocked, or a provider turn event on its Bus request arrived (see
    /// `accept_callback`). `send --async` compares it with a request's submission
    /// revision to tell that the agent worked on the message and then went
    /// idle, without relying on reply capture.
    #[serde(default)]
    pub(crate) busy_revision: u64,
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
    /// Set only on MASTER agents: the work room this agent orchestrates, bound
    /// at creation for the agent's whole life (its system prompt names it).
    /// `None` only for a saved orchestrator whose room failed the load checks
    /// in `ensure_master_room`; Bus never creates one.
    #[serde(default)]
    pub(crate) orchestrates: Option<RoomId>,
    #[serde(default)]
    pub(crate) compactions: Compactions,
    /// `agent clear` reset the provider context: the next callback that names a
    /// different provider session rebinds this agent to it instead of being rejected.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) session_reset_pending: bool,
    /// When the agent entered its current `status`, from status polls. Stall
    /// detection measures how long an agent has sat idle on a request.
    #[serde(default)]
    pub(crate) status_since_ms: u64,
    /// When the agent's status was last polled. The model has no clock of its
    /// own, so a submission is timed by the poll that preceded it.
    #[serde(default)]
    pub(crate) observed_at_ms: u64,
    /// Why the native server last refused to type a queued request into this
    /// agent (see `rejection_reason`); cleared once a submission is confirmed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) delivery_rejection: Option<String>,
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
    /// The last sign of life for this request since Bus typed it: the status
    /// poll before submission, then every accepted provider callback.
    /// Stall detection counts idle time from here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) progress_at_ms: Option<u64>,
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
    ReservedAgentName,
    OrchestratorOutsideMaster(AgentId),
    NotOrchestratable(RoomId),
    RoomAlreadyOrchestrated { room: RoomId, agent: AgentId },
    OrchestratorAlreadyBound(AgentId),
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
            Self::ReservedAgentName => write!(
                formatter,
                "The name {HUMAN_RECIPIENT} is reserved for messages to the human"
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
                "Room {} already has orchestrator agent {}; a work room has at most one orchestrator",
                room.0, agent.0
            ),
            Self::OrchestratorAlreadyBound(agent) => write!(
                formatter,
                "Agent {} already orchestrates a room; an orchestrator keeps its room for life",
                agent.0
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

fn deserialize_notices<'de, D>(deserializer: D) -> Result<Vec<Prompt>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let mut notices = Vec::<Prompt>::deserialize(deserializer)?;
    notices.retain(|notice| notice.author != Author::Bus);
    Ok(notices)
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
        let name = agent_name(name)?;
        let neighbors = || {
            self.agents
                .values()
                .filter(|agent| agent.room_id == room_id)
        };
        let color = colors::next_agent_color(neighbors().map(|agent| agent.color));
        let accessible_color =
            colors::next_accessible_agent_color(neighbors().map(|agent| agent.accessible_color));
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
                busy_revision: 0,
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
                status_since_ms: 0,
                observed_at_ms: 0,
                delivery_rejection: None,
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
        let name = agent_name(name)?;
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
        for agent in self.agents_deleted_with_room(id) {
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
            // A room's orchestrator lives in MASTER but takes messages from
            // the room it orchestrates, such as a worker saying it is blocked.
            if agent.room_id != room && agent.orchestrates != Some(room) {
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
                    progress_at_ms: None,
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
        let submitted_at_ms = agent.observed_at_ms;
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
        // Starts the idle clock for stall detection at the submission.
        request_state.progress_at_ms = Some(submitted_at_ms);
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
                agent.delivery_rejection = Some(rejection_reason(&message).into());
                agent.actionable_error = Some(message);
            }
            SubmissionOutcome::Confirmed {
                provider_session_id,
                provider_turn_id,
            } => {
                if let Some(agent) = self.agents.get_mut(&agent_id) {
                    agent.delivery_rejection = None;
                }
                let Some(request_state) = self.requests.get_mut(&request) else {
                    return Err(ModelError::UnknownRequest(request));
                };
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

    /// Posts an agent's message to the Human in `room`, delivered to no agent.
    /// Orchestrators report this way when no Human message is open, so the
    /// report reaches the room's history instead of only their terminal.
    pub(crate) fn post_to_human(
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
        if agent_state.status != status {
            agent_state.status_since_ms = now_ms;
        }
        agent_state.status = status;
        agent_state.status_revision = revision;
        if matches!(status, RuntimeStatus::Working | RuntimeStatus::Blocked) {
            agent_state.busy_revision = revision;
        }
        agent_state.observed_at_ms = now_ms;
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

    /// Why `request` is stuck, once it has made no progress for a grace
    /// period, or `None`. A Working or Blocked agent is never stalled: a long
    /// turn is normal, and a blocked one waits on an answer, not on Bus (see
    /// `blocked_unanswered`). Recovery stays explicit (`request recover`);
    /// this only reports.
    pub(crate) fn stall_reason(&self, request: &Request, now_ms: u64) -> Option<String> {
        // A member typed into another request's turn shares that turn's fate.
        let lead = match request.group {
            Some(lead) => self.requests.get(&lead).unwrap_or(request),
            None => request,
        };
        let agent = self.agents.get(&lead.agent_id)?;
        let since = |grace: u64, from: u64| now_ms.saturating_sub(from) >= grace;
        if matches!(
            lead.phase,
            RequestPhase::Completed | RequestPhase::Abandoned
        ) {
            return None;
        }
        if matches!(
            agent.status,
            RuntimeStatus::Working | RuntimeStatus::Blocked
        ) {
            return None;
        }
        match lead.phase {
            RequestPhase::Completed | RequestPhase::Abandoned => None,
            RequestPhase::Queued => since(
                QUEUED_STALL_MS,
                lead.prompt.submitted_at_ms.max(agent.status_since_ms),
            )
            .then(|| {
                agent
                    .delivery_rejection
                    .clone()
                    .or_else(|| super::diagnostics::wait_reason(agent).map(Into::into))
                    .unwrap_or_else(|| "not_submitted".into())
            }),
            RequestPhase::Submitting | RequestPhase::Active => {
                let progress = lead.progress_at_ms.unwrap_or(agent.status_since_ms);
                if agent.status != RuntimeStatus::Idle
                    || !since(DELIVERED_STALL_MS, progress.max(agent.status_since_ms))
                {
                    return None;
                }
                Some(
                    if lead.phase == RequestPhase::Submitting {
                        "submission_unconfirmed"
                    } else if !lead.trusted_start_bound {
                        "no_start_hook"
                    } else {
                        // The turn started and the agent is idle, yet no
                        // final reply matched the provider's transcript.
                        "transcript_unmatched"
                    }
                    .into(),
                )
            }
        }
    }

    /// Whether `request`'s agent has sat blocked (a dialog, question or trust
    /// prompt) past `BLOCKED_STALL_MS` while the request is unsettled. The
    /// agent asks for help once per blocked episode; this surfaces a block
    /// nobody answered in `message status`, without ending `send --async`,
    /// which waits on through a block by design.
    pub(crate) fn blocked_unanswered(&self, request: &Request, now_ms: u64) -> bool {
        if matches!(
            request.phase,
            RequestPhase::Completed | RequestPhase::Abandoned
        ) {
            return false;
        }
        self.agents.get(&request.agent_id).is_some_and(|agent| {
            let from = agent
                .status_since_ms
                .max(request.progress_at_ms.unwrap_or(0))
                .max(request.prompt.submitted_at_ms);
            agent.status == RuntimeStatus::Blocked
                && now_ms.saturating_sub(from) >= BLOCKED_STALL_MS
        })
    }

    /// Whether `request`'s agent took the message and finished its turn: the
    /// agent was seen Working or Blocked, or its provider reported the turn,
    /// after Bus submitted the message, and has been seen Idle since. A captured reply also counts. Status
    /// transitions decide it, not reply capture, which can miss a reply.
    pub(crate) fn turn_ended(&self, request: &Request) -> bool {
        if request.phase == RequestPhase::Completed {
            return true;
        }
        // Submitting counts too: an uncertain submit stays Submitting until a
        // hook binds it, while the agent may already be working on it.
        if !matches!(
            request.phase,
            RequestPhase::Submitting | RequestPhase::Active
        ) {
            return false;
        }
        self.agents.get(&request.agent_id).is_some_and(|agent| {
            agent.busy_revision > request.submission_status_revision
                && agent.status == RuntimeStatus::Idle
                && agent.status_revision > agent.busy_revision
        })
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
    /// if any, is in the terminal. Bus says nothing: it never authors a message,
    /// and a waiting sender sees the request `abandoned`.
    pub(crate) fn release_unbound_request(
        &mut self,
        agent: AgentId,
        now_ms: u64,
    ) -> Option<RequestId> {
        let request = self.unbound_request(agent)?.id;
        self.abandon_current_request(request, now_ms).ok()?;
        tracing::info!(
            event = "bus.message.recovered",
            request_id = request.0,
            agent_id = agent.0,
            reason = "never_started",
            "Unstarted request released"
        );
        Some(request)
    }

    pub(crate) fn accept_callback(&mut self, callback: ProviderCallback) -> CallbackDisposition {
        let agent = callback.agent_id;
        let occurred_at_ms = callback.occurred_at_ms;
        let current = self.agents.get(&agent).and_then(|a| a.current_request);
        let disposition = self.accept_callback_unrecorded(callback);
        if !matches!(disposition, CallbackDisposition::Rejected(_)) {
            if let Some(request) = current.and_then(|id| self.requests.get_mut(&id)) {
                let progress = request.progress_at_ms.get_or_insert(occurred_at_ms);
                *progress = (*progress).max(occurred_at_ms);
            }
        }
        // A provider turn event on a Bus request proves the agent was busy
        // with it, even when the turn started and ended between two status
        // polls that only ever saw Idle. Recording the edge here lets
        // `turn_ended` (and so `send --async`) finish on the next Idle poll.
        if matches!(
            disposition,
            CallbackDisposition::AcceptedBinding
                | CallbackDisposition::AcceptedContinuation
                | CallbackDisposition::AcceptedSteering
                | CallbackDisposition::AcceptedProgress
                | CallbackDisposition::AcceptedPendingSettlement
        ) {
            self.record_busy_edge(agent);
        }
        disposition
    }

    /// Marks `agent` busy at a fresh status revision, as a Working poll would.
    fn record_busy_edge(&mut self, agent: AgentId) {
        let revision = self.next_status_revision;
        self.next_status_revision = self.next_status_revision.saturating_add(1);
        if let Some(agent) = self.agents.get_mut(&agent) {
            agent.busy_revision = revision;
        }
    }

    fn accept_callback_unrecorded(&mut self, callback: ProviderCallback) -> CallbackDisposition {
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
        // Cursor opens a generation of its own when a background shell finishes.
        // That prompt is a task notice, not a room message. Record the turn as
        // unrelated so its later stop is neither the room reply nor WrongTurn.
        if callback
            .prompt_payload
            .as_deref()
            .is_some_and(callbacks::cursor_reply::is_background_task_notice)
        {
            self.unrelated_provider_turns.extend(turn_key);
            return CallbackDisposition::Rejected(CallbackRejection::UnrelatedTurn);
        }
        if self.is_unrelated_turn(&callback) {
            return CallbackDisposition::Rejected(CallbackRejection::UnrelatedTurn);
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

/// How long a queued message may wait on an agent that is not working before
/// it counts as stalled. Delivery normally starts within a status tick of the
/// agent going idle, so two minutes means something refuses it.
pub(crate) const QUEUED_STALL_MS: u64 = 2 * 60 * 1000;
/// How long a typed message may sit with its agent idle and no hook progress.
/// Longer than the queued grace: providers can take a while to report the
/// turn start or the final reply after the screen goes idle.
pub(crate) const DELIVERED_STALL_MS: u64 = 3 * 60 * 1000;
/// How long an agent may stay blocked (a dialog, question or trust prompt)
/// before `message status` reports `blocked_unanswered`. Answering takes a
/// person or the orchestrator a moment; five minutes untouched means nobody
/// saw it.
pub(crate) const BLOCKED_STALL_MS: u64 = 5 * 60 * 1000;

/// The stall reason for a native refusal to type a request. The native
/// server (`src/app/api/agents.rs`) shares one `agent_not_ready` code for
/// several causes, so the message tells them apart.
pub(crate) fn rejection_reason(message: &str) -> &'static str {
    if message.contains("input box is not empty") || message.contains("composer is not empty") {
        "input_box_not_empty"
    } else if message.contains("blocked") {
        "agent_blocked"
    } else if message.contains("not ready") || message.contains("not an active") {
        "agent_not_ready"
    } else {
        "delivery_rejected"
    }
}

fn agent_name(name: &str) -> Result<String, ModelError> {
    let name = normalized_name(name)?;
    if name.eq_ignore_ascii_case(HUMAN_RECIPIENT) {
        return Err(ModelError::ReservedAgentName);
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
#[path = "tests/types_test.rs"]
mod tests;
