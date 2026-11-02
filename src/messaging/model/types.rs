//! Persisted DTOs, identities and payload representation.
use super::payload::payload_matches;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

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

/// The name every Bus-generated notice is shown under, in chat and in what
/// agents receive.
pub(crate) const BUS_SENDER: &str = "bus";

/// Who wrote a prompt. Saved JSON is `"human"`, `"bus"` or `{"agent":N}`.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Author {
    /// Saved as `"human"`; renaming the value would break existing history.
    #[serde(rename = "human")]
    Developer,
    Agent(AgentId),
    /// Bus itself, shown as the grey sender "bus": every notice Bus generates
    /// (a blocked dialog, a compaction limit, a deleted agent's
    /// open terminal) is signed this way, never with an agent's name.
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
    #[serde(default = "developer_author")]
    pub(crate) author: Author,
    pub(crate) text: String,
    pub(crate) files: Vec<PathBuf>,
    pub(crate) recipient_ids: AgentRecipients,
    pub(crate) submitted_at_ms: u64,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) idle_agent_ids: BTreeSet<AgentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) attention_origin: Option<super::AttentionOrigin>,
}

fn developer_author() -> Author {
    Author::Developer
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
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) missing_provider_binaries: BTreeSet<String>,
    pub(crate) draft: Draft,
    pub(crate) unread_count: u64,
    /// A message addressed to You arrived while this room was not visible.
    /// Separate from unread_count, which also includes peer traffic and replies.
    #[serde(default)]
    pub(crate) unseen_to_developer: bool,
    pub(crate) latest_prompt: Option<Prompt>,
    /// Messages to the developer with no agent recipients, including Bus's
    /// own notices.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) notices: Vec<Prompt>,
    pub(crate) latest_replies: BTreeMap<AgentId, Reply>,
    #[serde(default)]
    pub(crate) deletion_pending: bool,
    #[serde(default)]
    pub(crate) kind: RoomKind,
    /// The developer's per-room sound notification choice. Unset rooms follow their
    /// kind: MASTER rings, work rooms stay silent. Read it with `sound_enabled`.
    #[serde(default)]
    pub(crate) sound: Option<bool>,
    /// The system sound this room rings with, by name; None is Bus's own ding.
    #[serde(default)]
    pub(crate) sound_name: Option<String>,
    /// Per MASTER agent, the provider turn its last final ended, so a report
    /// is compared only with the messages its own turn sent.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) report_turns: BTreeMap<AgentId, ReportTurn>,
    #[serde(
        default,
        skip_serializing_if = "super::disk_space::DiskAlerts::is_empty"
    )]
    pub(super) disk_alerts: super::disk_space::DiskAlerts,
}

/// A provider turn that ended with a final callback, and the notice ids it
/// spans: notices from `first_notice` on were posted during or after it, and
/// those from `next_notice` on after its last final.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ReportTurn {
    pub(crate) session: Option<String>,
    pub(crate) turn: String,
    pub(crate) first_notice: u64,
    pub(crate) next_notice: u64,
}

impl Room {
    pub(crate) fn sound_enabled(&self) -> bool {
        self.sound.unwrap_or(self.kind == RoomKind::Master)
    }
}

/// A session's rooms are units of work plus exactly one MASTER room, where the
/// orchestrator agents of those rooms live and talk to the developer.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RoomKind {
    #[default]
    Work,
    Master,
}

pub(crate) const MASTER_ROOM_NAME: &str = "MASTER";
/// The `send --to` selector for a message to the developer; no agent may take it.
pub(crate) const DEVELOPER_RECIPIENT: &str = "human";

pub(crate) fn is_developer_recipient(selector: &str) -> bool {
    selector.eq_ignore_ascii_case("you") || selector.eq_ignore_ascii_case(DEVELOPER_RECIPIENT)
}
/// Never produced by the ID allocator, which starts at 1, so adding MASTER to a
/// saved session neither collides with nor renumbers anything.
pub(super) const MASTER_ROOM_ID: RoomId = RoomId(0);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct AgentStartup {
    pub(crate) command: String,
    pub(crate) started_at_ms: u64,
    /// The startup diagnostic has been reported, including recoverable shell waits.
    pub(crate) reported: bool,
    /// The native attempt's terminal, independent of a later runtime rebind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) native_terminal_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) recovery: Option<StartupRecovery>,
    /// Conclusive failure, distinct from a recoverable shell wait.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) failure: Option<StartupFailure>,
}

pub(crate) const AGENT_STARTUP_FAILED: &str = "agent_startup_failed";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct StartupFailure {
    pub(crate) reason: String,
    pub(crate) recovery: String,
}

impl StartupFailure {
    pub(crate) fn notice(&self) -> String {
        format!("Startup failed: {}. {}", self.reason, self.recovery)
    }
}

/// Authority to retry only a provider command proven never to have reached input.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum StartupRecovery {
    Unsubmitted {
        args: Vec<String>,
    },
    /// Persisted before requesting input; uncertain outcomes cannot replay it.
    Requested,
}

/// Where an agent's blocked notice went.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DialogNoticeTo {
    /// Queued to the room's orchestrator as this request.
    Orchestrator(RequestId),
    Developer,
}

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
    /// Observed shell boundary for this launch/session, saved with callbacks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) claude_shells: Option<ClaudeShells>,
    /// Only a new launch is watched; successful providers are never cleaned
    /// up merely because they later become unavailable. Survives restart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) startup: Option<AgentStartup>,
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
    /// Who was told about the reported dialog, so its resolution follows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) dialog_notice_to: Option<DialogNoticeTo>,
    /// Identity-bound native UI episode reserved before an automatic answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) automatic_dialog_answer: Option<String>,
    pub(crate) actionable_error: Option<String>,
    /// The update failure the agent's terminal reported, shown as
    /// `actionable_error`. Saved, so the error still clears once the update is
    /// over after a coordinator restart, and only that error is cleared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) update_error: Option<String>,
    /// The missing-directory warning Bus owns, so recovery clears only this
    /// warning and a later callback cannot silently erase it for good.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) working_directory_error: Option<String>,
    /// A shared provider outage holds work until an explicit room resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) missing_binary: Option<String>,
    /// Delivery/correlation owner. A completed lead can remain here briefly
    /// while unconfirmed steering is matched, without withholding its answer.
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) unavailable_orchestration_target: Option<RoomId>,
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
    /// The session replaced by the last accepted reset. Persisted with the new
    /// identity so retained/late Grok observations cannot poison it on replay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) superseded_session_id: Option<String>,
    /// `agent interrupt` pressed the provider's stop key: the next idle status
    /// closes the running turn and the queue as interrupted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) interrupt_pending: bool,
    /// When the agent entered its current `status`, or its visible dialog
    /// last closed while Idle. Recovery deadlines counts usable idle time.
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
    /// What the agent's terminal showed when its provider last exited on its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) exit_snapshot: Option<ExitSnapshot>,
}

/// Inventory at the current request's boundary. Shells already present there
/// cannot become that request's work merely because Bus restarted.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ClaudeShells {
    pub(crate) owner: Option<RequestId>,
    pub(crate) inherited: BTreeSet<String>,
    pub(crate) running: BTreeSet<String>,
}

/// The screen Bus saved when an agent's provider exited without Bus asking,
/// before its terminal was closed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ExitSnapshot {
    /// The launch whose exit this is; one snapshot per launch.
    pub(crate) launch_id: Option<String>,
    pub(crate) terminal_id: String,
    /// The saved screen and scrollback, or None when the pane could not be read.
    pub(crate) path: Option<PathBuf>,
    /// Why nothing was saved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    /// The last non-blank lines, shown inline in exit notices.
    pub(crate) tail: String,
    pub(crate) at_ms: u64,
    /// Whether Bus closed the terminal once the snapshot was durable: None
    /// until it tried, false when it left the terminal open.
    pub(crate) terminal_closed: Option<bool>,
}

impl ExitSnapshot {
    /// The screen is saved and Bus has yet to close the terminal.
    pub(crate) fn close_pending(&self) -> bool {
        self.path.is_some() && self.terminal_closed.is_none()
    }

    /// The exit notices' lines about this snapshot.
    pub(crate) fn notice(&self) -> String {
        let Some(path) = &self.path else {
            return format!(
                "Exit snapshot unavailable: {}",
                self.error.as_deref().unwrap_or("unknown error")
            );
        };
        let terminal = match self.terminal_closed {
            Some(true) => " (Bus closed its terminal)",
            Some(false) => " (its terminal is still open)",
            None => "",
        };
        format!(
            "Exit snapshot: {}{terminal}.\nLast lines:\n{}",
            path.display(),
            self.tail
        )
    }
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

/// A request's durable answer, independent of its provider turn's pending final.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct CompletedReply {
    pub(crate) content: PendingFinal,
    /// A turn final is shared by its outstanding group; a recipient answer is individual.
    pub(crate) group: Option<RequestId>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum DialogNoticeDelivery {
    /// Saved before native I/O; a restart may lose its outcome entirely.
    Unconfirmed {
        since_ms: u64,
    },
    Confirmed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct QueuedRejection {
    /// The first definite refusal, not renewed by retries or unrelated turns.
    pub(crate) at_ms: u64,
    pub(crate) reason: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Request {
    pub(crate) id: RequestId,
    pub(crate) room_id: RoomId,
    pub(crate) agent_id: AgentId,
    pub(crate) prompt: Prompt,
    /// Sender's (name, home room) at send time; survives rename and deletion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sender_identity: Option<(String, RoomId)>,
    /// Delivery/turn lifecycle; a replied anchor stays active until its turn settles.
    pub(crate) phase: RequestPhase,
    /// A queued request closed before typing, retained for status and restart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) failure_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) queued_rejection: Option<QueuedRejection>,
    pub(crate) expected_launch_id: Option<String>,
    pub(crate) submission_boundary: Option<u64>,
    pub(crate) submission_status_revision: u64,
    pub(crate) provider_session_id: Option<String>,
    pub(crate) provider_turn_id: Option<String>,
    pub(crate) provider_prompt_id: Option<String>,
    #[serde(default)]
    pub(crate) trusted_start_bound: bool,
    pub(crate) uncertain_outcome: bool,
    /// Native launch/draft/recovery detail belongs to this send, beyond agent changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) submission_error: Option<String>,
    /// A blocked-dialog episode outlives request/group settlement. Keep its
    /// native receipt and write clock independently of those transitions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) dialog_delivery: Option<DialogNoticeDelivery>,
    /// The provider turn's candidate final, not the request's completion result.
    pub(crate) pending_final: Option<PendingFinal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) completed_reply: Option<CompletedReply>,
    pub(crate) completed_at_ms: Option<u64>,
    /// Sent with `--queue`: waits for an idle agent and gets its own turn.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) queue_only: bool,
    /// Bus-created input (such as a reply notice or first-turn system prompt),
    /// retained through startup failure rather than closed as a sender's send.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) bus_generated: bool,
    /// The lead request of the group this one joined, by being typed into the
    /// lead's running turn or coalesced into its prompt. Outstanding requests
    /// share the eventual final; completed replies remain independent.
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
    /// Recovery deadlines counts idle time from here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) progress_at_ms: Option<u64>,
    /// First settled observation after this turn, while reply capture catches up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) turn_ended_at_ms: Option<u64>,
    /// A provider paused without an answer for background work that will resume it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) awaiting_background: bool,
}

impl Request {
    pub(crate) fn is_error_report(&self) -> bool {
        self.prompt.author == Author::Bus
            && self
                .prompt
                .text
                .starts_with(crate::utils::error_pipe::NOTICE_PREFIX)
    }

    pub(crate) fn matches_callback_payload(&self, payload: &str) -> bool {
        match &self.submitted_payload {
            Some(typed) => payload_matches(payload, typed),
            None => self.prompt.matches_callback_payload(payload),
        }
    }

    pub(crate) fn reply(&self) -> Option<&PendingFinal> {
        self.completed_reply.as_ref().map(|reply| &reply.content)
    }

    pub(crate) fn reply_key(&self) -> RequestId {
        self.completed_reply.as_ref().map_or_else(
            || self.group.unwrap_or(self.id),
            |reply| reply.group.unwrap_or(self.id),
        )
    }

    pub(crate) fn settled(&self) -> bool {
        self.completed_reply.is_some()
            || matches!(
                self.phase,
                RequestPhase::Completed | RequestPhase::Abandoned
            )
    }
}

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SubmissionOutcome {
    /// No input was sent: retain the message until its composer is available.
    ComposerBusy,
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
    Final {
        text: String,
    },
    Error {
        message: String,
        /// The provider's own description of the error that ended the turn.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_error: Option<String>,
    },
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

/// How long a queued message may wait on an agent that is not working before
/// it needs recovery. Delivery normally starts within a status tick of the
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
