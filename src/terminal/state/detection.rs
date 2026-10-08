#[cfg(windows)]
use std::time::Duration;
use std::time::Instant;

#[cfg(test)]
use super::EffectiveStateChange;

use super::{EffectivePresentation, RecentAgentProcessExit, TerminalState, TerminalStateMutation};
use crate::detect::{Agent, AgentState};

impl TerminalState {
    pub fn set_detected_agent_process_at(
        &mut self,
        agent: Agent,
        now: Instant,
    ) -> TerminalStateMutation {
        let starts_acquisition = self.recent_agent_process_exit.is_none();
        let mutation = self.set_detected_state_with_screen_signals_at(
            Some(agent),
            AgentState::Unknown,
            false,
            false,
            now,
        );
        if starts_acquisition {
            self.agent_process_acquisition_pending = true;
        }
        mutation
    }

    pub(crate) fn finish_agent_process_acquisition(&mut self) -> bool {
        let reached_idle = self.agent_process_acquisition_pending && self.state == AgentState::Idle;
        let suppress_completion = reached_idle && self.recent_agent_process_exit.is_none();
        if reached_idle {
            self.agent_process_acquisition_pending = false;
        }
        suppress_completion
    }

    #[cfg(windows)]
    pub(crate) fn agent_process_exited_within(&self, now: Instant, max_age: Duration) -> bool {
        self.recent_agent_process_exit
            .is_some_and(|exit| now.saturating_duration_since(exit.observed_at) <= max_age)
    }

    #[cfg(test)]
    pub fn set_detected_state(
        &mut self,
        agent: Option<Agent>,
        fallback_state: AgentState,
    ) -> Option<EffectiveStateChange> {
        self.set_detected_state_with_visible_blocker(agent, fallback_state, false, false)
    }

    #[cfg(test)]
    pub fn set_detected_state_with_visible_blocker(
        &mut self,
        agent: Option<Agent>,
        fallback_state: AgentState,
        visible_blocker: bool,
        process_exited: bool,
    ) -> Option<EffectiveStateChange> {
        self.set_detected_state_with_screen_signals_at(
            agent,
            fallback_state,
            visible_blocker,
            process_exited,
            Instant::now(),
        )
        .effective_state_change
    }

    pub fn set_detected_state_with_screen_signals_at(
        &mut self,
        agent: Option<Agent>,
        fallback_state: AgentState,
        _visible_blocker: bool,
        process_exited: bool,
        now: Instant,
    ) -> TerminalStateMutation {
        let previous_agent_label = self.effective_agent_label().map(str::to_string);
        let previous_known_agent = self.effective_known_agent();
        let previous_state = self.state;
        let previous_presentation = EffectivePresentation::default();
        let previous_session = self.current_session_identity_for_persistence();
        let agent_released =
            process_exited && (previous_agent_label.is_some() || self.agent_name.is_some());
        self.detected_agent = agent;
        if let Some(agent) = agent {
            let agent_label = crate::detect::agent_label(agent);
            self.reconcile_agent_name_owner(agent_label, None);
        }
        self.fallback_state = fallback_state;
        if process_exited {
            if let Some(agent) = agent {
                self.recent_agent_process_exit = Some(RecentAgentProcessExit {
                    agent,
                    observed_at: now,
                });
            }
            if self
                .persisted_agent_session
                .as_ref()
                .is_some_and(|session| crate::detect::parse_agent_label(&session.agent) == agent)
            {
                self.persisted_agent_session = None;
            }
        } else if agent.is_some() {
            self.recent_agent_process_exit = None;
        }
        if agent_released {
            self.clear_agent_name();
        }
        TerminalStateMutation {
            effective_state_change: self.recompute_effective_state(
                previous_agent_label,
                previous_known_agent,
                previous_state,
                previous_presentation,
                now,
            ),
            session_ref_changed: previous_session
                != self.current_session_identity_for_persistence(),
            agent_released,
        }
    }

    pub fn clear_agent_runtime_identity_after_respawn(&mut self) {
        self.detected_agent = None;
        self.fallback_state = AgentState::Unknown;
        self.persisted_agent_session = None;
        self.state = AgentState::Unknown;
        self.last_agent_state_change_seq = None;
        self.launch_argv = None;
        self.respawn_shell_on_exit = false;
        self.recent_agent_process_exit = None;
        self.agent_process_acquisition_pending = false;
        self.pending_agent_resume_plan = None;
        self.clear_agent_name();
    }
}

pub(crate) fn stabilize_agent_detection(detection: crate::detect::AgentDetection) -> AgentState {
    detection.state
}
