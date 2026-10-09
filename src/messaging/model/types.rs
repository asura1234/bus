//! Persisted DTOs, identities and payload representation.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

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

pub(crate) use crate::agents::providers::ProviderKind as Provider;

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
    /// Render a code-driven compaction notice as one plain chat line.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) compaction_limit_notice: bool,
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

    pub(super) fn matches_callback_payload(&self, payload: &str) -> bool {
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
pub(super) const MASTER_ROOM_ID: RoomId = RoomId(0);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename = "Agent")]
pub(crate) struct RoomAgent {
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
    /// The option chosen through Bus, so the closing notice can name it.
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
    /// The launch and spool sequence of the last compaction hook counted. State
    /// is saved before the envelope is unlinked, so a crash between the two
    /// replays it; identical hooks reuse one callback id, so the sequence is the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_compaction_callback: Option<(String, u64)>,
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
    /// Saved with the notice; clears only on context reset or a higher limit.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) limit_notice_sent: bool,
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
    /// A queued request closed before typing, retained for status and restart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) failure_reason: Option<String>,
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
    /// First settled observation after this turn, while reply capture catches up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) turn_ended_at_ms: Option<u64>,
    /// A provider explicitly paused this turn for background work that will resume it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) awaiting_background: bool,
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

    pub(super) fn settled(&self) -> bool {
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

/// Allow completion hooks and transcript writes to arrive after the idle poll.
pub(crate) const TURN_SETTLE_MS: u64 = 5_000;

/// Local wall-clock `HH:MM` of a millisecond timestamp, for coalesced prompts.
pub(super) fn clock(at_ms: u64) -> String {
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
pub(super) fn payload_matches(payload: &str, typed: &str) -> bool {
    let payload = normalized_payload(payload);
    let typed = normalized_payload(typed);
    if payload == typed {
        return true;
    }
    let (length, images) =
        crate::agents::providers::claude_code::hooks::claude_image_placeholders(&payload);
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
    let remaining = remaining.trim_end_matches(|character: char| character.is_ascii_whitespace());
    images > 0 && images == lifted && &payload[length..] == remaining
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

fn deserialize_notices<'de, D>(deserializer: D) -> Result<Vec<Prompt>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let mut notices = Vec::<Prompt>::deserialize(deserializer)?;
    notices.retain(|notice| notice.author != Author::Bus);
    Ok(notices)
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
