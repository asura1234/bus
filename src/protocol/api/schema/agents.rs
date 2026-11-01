use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::common::{AgentStatus, ReadFormat, ReadSource};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentReadParams {
    pub target: String,
    pub source: ReadSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<u32>,
    #[serde(default)]
    pub format: ReadFormat,
    #[serde(default = "super::common::default_true")]
    pub strip_ansi: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentSendKeysParams {
    pub target: String,
    pub keys: Vec<String>,
}

/// Provider-independent observation, including terminals with no agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentObserveParams {
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentPressParams {
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_terminal_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub select: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<String>,
    #[serde(default)]
    pub submit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TerminalObservation {
    pub terminal_id: String,
    pub pane_id: String,
    pub content_revision: u64,
    pub text: String,
    pub grid: super::driver::Grid,
    pub focus: Option<(usize, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TerminalPressResult {
    pub keys: Vec<String>,
    pub text_written: bool,
    /// One navigation step was written; observe its redraw before continuing.
    pub selection_pending: bool,
    pub observation: TerminalObservation,
}

/// Observing is read-only unless the caller explicitly opens a queued question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentDialogObserveParams {
    pub target: String,
    #[serde(default, skip_serializing_if = "super::is_false")]
    pub open_pending_question: bool,
}

/// One numbered option of a choice dialog on an agent's screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentDialogOption {
    pub number: u32,
    pub label: String,
    pub selected: bool,
    /// An inline text field: answer it with text, not by choosing it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub free_text: bool,
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum AgentDialogKind {
    #[default]
    Choice,
    Question,
}

/// A choice dialog or a focused free-text question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentDialog {
    #[serde(default)]
    pub kind: AgentDialogKind,
    /// The question or title above the options.
    pub text: String,
    pub options: Vec<AgentDialogOption>,
    /// The key hint below the options, such as `Esc to cancel`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Identifies the question and options, whichever option is selected.
    pub id: String,
    /// Identifies this exact dialog, including its selected option.
    pub digest: String,
    /// A multi-question form's question tabs, then its Submit tab.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tabs: Vec<AgentDialogTab>,
    /// Claude's `Type something` row, also when a short pane scrolls it away.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_text: Option<AgentDialogFreeText>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentDialogFreeText {
    pub option: u32,
    /// It holds text someone typed, which Enter on the row submits.
    pub draft: bool,
    /// A multi-select checkbox, which Enter toggles.
    pub checkbox: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentDialogTab {
    pub label: String,
    pub answered: bool,
    /// The tab whose question the dialog shows.
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentDialogObservation {
    /// A queued Codex question is still collapsed, possibly awaiting its redraw.
    #[serde(default, skip_serializing_if = "super::is_false")]
    pub pending_question: bool,
    pub terminal_id: String,
    pub pane_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub content_revision: u64,
    /// `None` when no choice dialog is visible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialog: Option<AgentDialog>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentDialogChooseParams {
    pub target: String,
    pub expected_terminal_id: String,
    pub expected_pane_id: String,
    /// Checked when set; a launching agent has no bound session yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_session_id: Option<String>,
    pub expected_dialog_digest: String,
    pub option: u32,
    /// Automatic UI actions separate navigation from confirmation. Omission
    /// preserves ordinary choose behavior for existing callers.
    #[serde(default, skip_serializing_if = "AgentDialogChoiceAction::is_choose")]
    pub action: AgentDialogChoiceAction,
}

#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum AgentDialogChoiceAction {
    #[default]
    Choose,
    /// Send only navigation arrows, never confirmation.
    Focus,
    /// Send Enter only if the requested option is already visibly selected.
    Confirm,
}

impl AgentDialogChoiceAction {
    fn is_choose(&self) -> bool {
        *self == Self::Choose
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentDialogAnswerParams {
    pub target: String,
    pub expected_terminal_id: String,
    pub expected_pane_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_session_id: Option<String>,
    pub expected_dialog_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default)]
    pub skip: bool,
}

impl AgentDialogAnswerParams {
    pub(crate) fn validate_answer(text: Option<&str>, skip: bool) -> Result<(), &'static str> {
        if skip == text.is_some() {
            return Err("Provide exactly one of text or skip");
        }
        if text.is_some_and(|text| text.trim().is_empty()) {
            return Err("Answer text must not be blank");
        }
        if text.is_some_and(|text| {
            text.chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t')
        }) {
            return Err("Answer text contains terminal control characters");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentDialogChooseResult {
    pub written: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The keys sent, such as `["down", "enter"]`; empty when nothing was sent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<String>,
    pub observation: AgentDialogObservation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentWaitParams {
    pub target: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub until: Vec<AgentStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentPromptWaitOptions {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub until: Vec<AgentStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) submission_deadline: Option<std::time::Instant>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentRenameParams {
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentStartParams {
    pub name: String,
    pub kind: String,
    pub pane_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Validated absolute provider alias selected during launch preparation.
    /// Required for Grok and Kimi so shell startup cannot redirect the binary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    /// Validated Grok global home, applied again after shell startup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grok_home: Option<String>,
    /// Owner-only Kimi home containing Bus hooks and the session wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kimi_home: Option<String>,
    /// Startup timeout in milliseconds. Values must be greater than 3000 and at most 300000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// Retry an unsubmitted launch only in its retained, prompt-integrated terminal.
/// A separate method makes older servers reject the terminal ownership guard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentStartIfIdentityParams {
    pub expected_terminal_id: String,
    #[serde(flatten)]
    pub start: AgentStartParams,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentPromptParams {
    pub target: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<AgentPromptWaitOptions>,
}

/// A separate method makes older servers reject the safety guard, never ignore it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentPromptIfIdleParams {
    pub target: String,
    pub text: String,
    pub expected_terminal_id: String,
    pub expected_pane_id: String,
    pub expected_agent: String,
    pub expected_session_id: String,
    /// Also type into a working agent, as a person types while it works, so
    /// the provider takes the text into its running turn. A blocked agent is
    /// still refused.
    #[serde(default, skip_serializing_if = "super::is_false")]
    pub steer: bool,
}

/// First interactive Codex turn: its SessionStart hook is deferred until input.
/// This separate method requires the exact managed launch, never an arbitrary pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentPromptIfUnboundParams {
    pub target: String,
    pub text: String,
    pub expected_terminal_id: String,
    pub expected_pane_id: String,
    pub expected_managed_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentInfo {
    /// Native restore attempt, including a failure before process detection.
    /// Absent after interactive confirmation and on ordinary client reattach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_resume: Option<NativeResumeInfo>,
    /// Definite launch input failure before Enter, with no screen or message content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_input_error: Option<String>,
    pub terminal_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_title_stripped: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_agent: Option<String>,
    pub agent_status: AgentStatus,
    #[serde(default, skip_serializing_if = "super::is_false")]
    pub screen_detection_skipped: bool,
    /// The `id` of the numbered choice dialog waiting for an answer, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialog_id: Option<String>,
    /// Why Bus could not install the agent's own update; its update chooser
    /// then waits for a person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_error: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub state_labels: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    #[schemars(schema_with = "super::common::metadata_token_values_schema")]
    pub tokens: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session: Option<AgentSessionInfo>,
    pub workspace_id: String,
    pub tab_id: String,
    pub pane_id: String,
    pub focused: bool,
    #[serde(default, skip_serializing_if = "super::is_false")]
    pub launch_pending: bool,
    #[serde(default, skip_serializing_if = "super::is_false")]
    pub interactive_ready: bool,
    /// A current PTY hosts the expected provider process, with no deferred
    /// resume or pending launch. Saved identity and idle state alone do not
    /// make this true. Check agent_status separately before submitting input.
    #[serde(default, skip_serializing_if = "super::is_false")]
    pub native_ready: bool,
    #[serde(default)]
    pub state_change_seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground_cwd: Option<String>,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NativeResumeInfo {
    pub name: String,
    pub agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentSessionInfo {
    pub source: String,
    pub agent: String,
    pub kind: AgentSessionRefKind,
    pub value: String,
}

// API vocabulary stays independent from persisted provider session references.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentSessionRefKind {
    Id,
    Path,
}

/// Capture only the exact owned launch terminal; never follow a replacement pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentStartupSnapshotParams {
    pub pane_id: String,
    pub expected_terminal_id: String,
}

/// Native startup evidence, independent of coordinator readiness interpretation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentStartupSnapshot {
    pub pane_id: String,
    pub terminal_id: String,
    pub native_agent: Option<String>,
    pub native_name: Option<String>,
    pub native_status: AgentStatus,
    pub launch_pending: bool,
    pub interactive_ready: bool,
    /// Visible startup rows, including empty rows; no scrollback or request payloads.
    pub screen: String,
    pub osc_title: String,
    pub osc_progress: String,
    pub content_revision: u64,
    pub input_error: Option<String>,
}
