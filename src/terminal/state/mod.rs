mod detection;
mod managed_agent;
mod sessions;

pub(crate) use detection::stabilize_agent_detection;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use crate::detect::{Agent, AgentState};
use crate::terminal::TerminalId;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EffectivePresentation {
    pub title: Option<String>,
    pub display_agent: Option<String>,
    pub state_labels: HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManagedAgentPhase {
    Pending {
        ready_after: Option<Instant>,
        deadline: Instant,
        observed_expected: bool,
    },
    Blocked,
    Active,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ManagedAgent {
    kind: Agent,
    phase: ManagedAgentPhase,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveStateChange {
    pub previous_agent_label: Option<String>,
    pub previous_known_agent: Option<Agent>,
    pub previous_state: AgentState,
    pub previous_presentation: EffectivePresentation,
    pub agent_label: Option<String>,
    pub known_agent: Option<Agent>,
    pub state: AgentState,
    pub presentation: EffectivePresentation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct TerminalTitleChange {
    pub(crate) raw_changed: bool,
    pub(crate) stripped_changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TerminalStateMutation {
    pub effective_state_change: Option<EffectiveStateChange>,
    pub session_ref_changed: bool,
    pub agent_released: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentNameOwner {
    agent_label: String,
    session_ref: Option<crate::agent_resume::AgentSessionRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RecentAgentProcessExit {
    agent: Agent,
    observed_at: Instant,
}

/// Pure state for a server-owned terminal.
///
/// Agent identity comes from process and screen detection. Provider session
/// identity is the persisted session reported by `pane.report_agent_session`.
pub struct TerminalState {
    pub id: TerminalId,
    pub cwd: PathBuf,
    pub detected_agent: Option<Agent>,
    pub fallback_state: AgentState,
    pub persisted_agent_session: Option<crate::agent_resume::PersistedAgentSession>,
    pub terminal_title: Option<String>,
    pub manual_label: Option<String>,
    pub agent_name: Option<String>,
    agent_name_owner: Option<AgentNameOwner>,
    managed_agent: Option<ManagedAgent>,
    managed_agent_launch_session: Option<crate::agent_resume::PersistedAgentSession>,
    session_report_sequences: HashMap<String, u64>,
    pub state: AgentState,
    pub last_agent_state_change_seq: Option<u64>,
    pub revision: u64,
    pub launch_argv: Option<Vec<String>>,
    pub respawn_shell_on_exit: bool,
    recent_agent_process_exit: Option<RecentAgentProcessExit>,
    agent_process_acquisition_pending: bool,
    pub pending_agent_resume_plan: Option<crate::agent_resume::AgentResumePlan>,
}

impl TerminalState {
    pub fn new(id: TerminalId, cwd: PathBuf) -> Self {
        Self {
            id,
            cwd,
            detected_agent: None,
            fallback_state: AgentState::Unknown,
            persisted_agent_session: None,
            terminal_title: None,
            manual_label: None,
            agent_name: None,
            agent_name_owner: None,
            managed_agent: None,
            managed_agent_launch_session: None,
            session_report_sequences: HashMap::new(),
            state: AgentState::Unknown,
            last_agent_state_change_seq: None,
            revision: 0,
            launch_argv: None,
            respawn_shell_on_exit: false,
            recent_agent_process_exit: None,
            agent_process_acquisition_pending: false,
            pending_agent_resume_plan: None,
        }
    }

    pub(crate) fn terminal_title_stripped(&self) -> Option<String> {
        self.terminal_title
            .as_deref()
            .and_then(super::stripped_terminal_title)
    }

    pub(crate) fn set_terminal_title(&mut self, title: Option<String>) -> TerminalTitleChange {
        if self.terminal_title == title {
            return TerminalTitleChange::default();
        }
        let previous_stripped = self.terminal_title_stripped();
        self.terminal_title = title;
        let stripped_changed = previous_stripped != self.terminal_title_stripped();
        if stripped_changed {
            self.revision = self.revision.wrapping_add(1);
        }
        TerminalTitleChange {
            raw_changed: true,
            stripped_changed,
        }
    }

    pub fn with_launch_argv(mut self, argv: Vec<String>) -> Self {
        self.launch_argv = Some(argv);
        self
    }

    pub fn with_pending_agent_resume_plan(
        mut self,
        plan: crate::agent_resume::AgentResumePlan,
    ) -> Self {
        self.pending_agent_resume_plan = Some(plan);
        self
    }

    pub fn effective_agent_label(&self) -> Option<&str> {
        self.recent_agent_process_exit
            .is_none()
            .then(|| self.detected_agent.map(crate::detect::agent_label))
            .flatten()
    }

    pub fn effective_known_agent(&self) -> Option<Agent> {
        self.effective_agent_label()
            .and_then(crate::detect::parse_agent_label)
    }

    pub(crate) fn unchanged_effective_state_change_at(&self, now: Instant) -> EffectiveStateChange {
        let _ = now;
        let agent_label = self.effective_agent_label().map(str::to_string);
        let known_agent = self.effective_known_agent();
        let state = self.state;
        let presentation = EffectivePresentation::default();
        EffectiveStateChange {
            previous_agent_label: agent_label.clone(),
            previous_known_agent: known_agent,
            previous_state: state,
            previous_presentation: presentation.clone(),
            agent_label,
            known_agent,
            state,
            presentation,
        }
    }

    pub fn set_manual_label(&mut self, label: String) {
        let label = label.trim().to_string();
        self.manual_label = (!label.is_empty()).then_some(label);
    }

    pub fn clear_manual_label(&mut self) {
        self.manual_label = None;
    }

    pub fn is_agent_terminal(&self) -> bool {
        self.agent_name.is_some() || self.effective_agent_label().is_some()
    }

    pub fn border_label(&self, show_agent_labels: bool) -> Option<String> {
        self.manual_label.clone().or_else(|| {
            show_agent_labels
                .then(|| self.effective_agent_label().map(str::to_string))
                .flatten()
        })
    }

    fn recompute_effective_state(
        &mut self,
        previous_agent_label: Option<String>,
        previous_known_agent: Option<Agent>,
        previous_state: AgentState,
        previous_presentation: EffectivePresentation,
        now: Instant,
    ) -> Option<EffectiveStateChange> {
        let _ = now;
        let state = self.fallback_state;
        let agent_label = self.effective_agent_label().map(str::to_string);
        let known_agent = self.effective_known_agent();
        let presentation = EffectivePresentation::default();
        if previous_agent_label == agent_label
            && previous_state == state
            && previous_presentation == presentation
        {
            return None;
        }
        self.state = state;
        Some(EffectiveStateChange {
            previous_agent_label,
            previous_known_agent,
            previous_state,
            previous_presentation,
            agent_label,
            known_agent,
            state,
            presentation,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    fn test_terminal() -> TerminalState {
        TerminalState::new(TerminalId::alloc(), "/tmp".into())
    }

    fn session_id(value: &str) -> crate::agent_resume::AgentSessionRef {
        crate::agent_resume::AgentSessionRef::id(value).unwrap()
    }

    include!("tests/detection_test.rs");
    include!("tests/process_exit_test.rs");
    include!("tests/sessions_test.rs");
    include!("tests/session_replacement_test.rs");
    include!("tests/managed_agent_test.rs");
}
