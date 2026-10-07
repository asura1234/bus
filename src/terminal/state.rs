use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

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

    #[cfg(any(windows, test))]
    pub(crate) fn agent_process_exited_within(&self, now: Instant, max_age: Duration) -> bool {
        self.recent_agent_process_exit
            .is_some_and(|exit| now.saturating_duration_since(exit.observed_at) <= max_age)
    }

    pub fn with_pending_agent_resume_plan(
        mut self,
        plan: crate::agent_resume::AgentResumePlan,
    ) -> Self {
        self.pending_agent_resume_plan = Some(plan);
        self
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
    pub fn set_detected_state_with_mutation(
        &mut self,
        agent: Option<Agent>,
        fallback_state: AgentState,
    ) -> TerminalStateMutation {
        self.set_detected_state_with_screen_signals_at(
            agent,
            fallback_state,
            false,
            false,
            Instant::now(),
        )
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

    pub fn set_persisted_agent_session(
        &mut self,
        session: crate::agent_resume::PersistedAgentSession,
    ) {
        self.persisted_agent_session = Some(session);
    }

    pub fn set_managed_agent_launch_session(
        &mut self,
        session: crate::agent_resume::PersistedAgentSession,
    ) {
        self.persisted_agent_session = Some(session.clone());
        self.managed_agent_launch_session = Some(session);
    }

    pub fn set_agent_session_ref(
        &mut self,
        source: String,
        agent_label: String,
        session_ref: Option<crate::agent_resume::AgentSessionRef>,
        seq: Option<u64>,
    ) -> Option<TerminalStateMutation> {
        self.set_agent_session_ref_for_session_start(source, agent_label, session_ref, seq, None)
    }

    pub fn set_agent_session_ref_for_session_start(
        &mut self,
        source: String,
        agent_label: String,
        session_ref: Option<crate::agent_resume::AgentSessionRef>,
        seq: Option<u64>,
        session_start_source: Option<String>,
    ) -> Option<TerminalStateMutation> {
        let session_ref = session_ref?;
        let known_agent = crate::detect::parse_agent_label(&agent_label);
        let process_present = known_agent.is_some()
            && self.detected_agent == known_agent
            && self.recent_agent_process_exit.is_none();
        if !self.accept_session_report(&source, seq) {
            return None;
        }
        if self.known_agent_label_conflicts_with_detected_agent(&agent_label) {
            return None;
        }
        let session_replacement_allowed = Self::session_report_allows_session_replacement(
            &source,
            &agent_label,
            session_start_source.as_deref(),
        );
        let replacing_identity_only_session =
            crate::detect::session_identity_only_integration(&source, &agent_label)
                && session_replacement_allowed
                && self.current_session_identity_for_persistence().is_some_and(
                    |(current_source, current_agent, current_kind, current_value)| {
                        current_source == source
                            && current_agent == agent_label
                            && current_kind == crate::agent_resume::AgentSessionRefKind::Id
                            && session_ref.kind == crate::agent_resume::AgentSessionRefKind::Id
                            && current_value != session_ref.value
                    },
                );
        if replacing_identity_only_session && !process_present {
            return None;
        }
        let owner_conflicts = self.current_session_owner_conflicts(&source, &agent_label);
        let foreground_takeover_allowed = owner_conflicts
            && self.foreground_agent_confirms_different_owner_takeover(
                &source,
                &agent_label,
                &session_ref,
                session_start_source.as_deref(),
            );
        if owner_conflicts && !foreground_takeover_allowed {
            return None;
        }
        if self
            .conflicting_same_owner_session_ref(
                &source,
                &agent_label,
                &session_ref,
                session_start_source.as_deref(),
            )
            .is_some()
        {
            return None;
        }

        let now = Instant::now();
        let previous_agent_label = self.effective_agent_label().map(str::to_string);
        let previous_known_agent = self.effective_known_agent();
        let previous_state = self.state;
        let previous_presentation = EffectivePresentation::default();
        let previous_session = self.current_session_identity_for_persistence();
        self.reconcile_agent_name_owner(&agent_label, Some(&session_ref));
        let persisted_session = crate::agent_resume::PersistedAgentSession {
            source,
            agent: agent_label,
            session_ref,
        };
        if self.managed_agent_launch_session.as_ref() == Some(&persisted_session) {
            self.managed_agent_launch_session = None;
        }
        self.persisted_agent_session = Some(persisted_session);
        let current_session = self.current_session_identity_for_persistence();
        Some(TerminalStateMutation {
            effective_state_change: self.recompute_effective_state(
                previous_agent_label,
                previous_known_agent,
                previous_state,
                previous_presentation,
                now,
            ),
            session_ref_changed: previous_session != current_session,
            agent_released: false,
        })
    }

    fn known_agent_label_conflicts_with_detected_agent(&self, agent_label: &str) -> bool {
        let Some(detected_agent) = self.detected_agent else {
            return false;
        };
        crate::detect::parse_agent_label(agent_label)
            .is_some_and(|reported| reported != detected_agent)
    }

    fn foreground_agent_confirms_different_owner_takeover(
        &self,
        source: &str,
        agent_label: &str,
        session_ref: &crate::agent_resume::AgentSessionRef,
        session_start_source: Option<&str>,
    ) -> bool {
        Self::session_start_source_is_recognized(session_start_source)
            && self.foreground_agent_confirms_session_owner(source, agent_label, session_ref)
    }

    fn foreground_agent_confirms_session_owner(
        &self,
        source: &str,
        agent_label: &str,
        session_ref: &crate::agent_resume::AgentSessionRef,
    ) -> bool {
        let Some(detected_agent) = self.detected_agent else {
            return false;
        };
        crate::detect::parse_agent_label(agent_label) == Some(detected_agent)
            && crate::agent_resume::plan(source, agent_label, session_ref).is_some()
    }

    fn accept_session_report(&mut self, source: &str, seq: Option<u64>) -> bool {
        let Some(seq) = seq else {
            return !self.session_report_sequences.contains_key(source);
        };
        if self
            .session_report_sequences
            .get(source)
            .is_some_and(|last_seq| seq <= *last_seq)
        {
            return false;
        }
        self.session_report_sequences
            .insert(source.to_string(), seq);
        true
    }

    fn current_session_identity_for_persistence(
        &self,
    ) -> Option<(
        String,
        String,
        crate::agent_resume::AgentSessionRefKind,
        String,
    )> {
        self.persisted_agent_session.as_ref().map(|session| {
            (
                session.source.clone(),
                session.agent.clone(),
                session.session_ref.kind,
                session.session_ref.value.clone(),
            )
        })
    }

    fn current_session_owner_conflicts(&self, source: &str, agent_label: &str) -> bool {
        self.current_session_identity_for_persistence().is_some_and(
            |(current_source, current_agent, _, _)| {
                current_source != source || current_agent != agent_label
            },
        )
    }

    fn conflicting_same_owner_session_ref(
        &self,
        source: &str,
        agent_label: &str,
        session_ref: &crate::agent_resume::AgentSessionRef,
        session_start_source: Option<&str>,
    ) -> Option<crate::agent_resume::AgentSessionRef> {
        self.current_session_identity_for_persistence().and_then(
            |(current_source, current_agent, current_kind, current_value)| {
                (current_source == source
                    && current_agent == agent_label
                    && current_kind == crate::agent_resume::AgentSessionRefKind::Id
                    && session_ref.kind == crate::agent_resume::AgentSessionRefKind::Id
                    && current_value != session_ref.value
                    && !Self::session_report_allows_session_replacement(
                        source,
                        agent_label,
                        session_start_source,
                    ))
                .then_some(crate::agent_resume::AgentSessionRef {
                    kind: current_kind,
                    value: current_value,
                })
            },
        )
    }

    fn session_report_allows_session_replacement(
        source: &str,
        agent_label: &str,
        session_start_source: Option<&str>,
    ) -> bool {
        matches!(
            (source, agent_label, session_start_source),
            (
                "herdr:claude",
                "claude",
                Some("clear" | "resume" | "compact")
            ) | (
                "herdr:codex",
                "codex",
                Some("startup" | "clear" | "resume" | "compact")
            ) | ("herdr:cursor", "cursor", Some("new"))
                | ("herdr:mastracode", "mastracode", Some("startup"))
                | ("herdr:hermes", "hermes", Some("startup" | "new" | "resume"))
                | ("herdr:opencode", "opencode", Some("select"))
                | ("herdr:pi", "pi", Some("new" | "resume" | "fork"))
                | (
                    "herdr:omp",
                    "omp",
                    Some("startup" | "new" | "resume" | "fork")
                )
                | (
                    "herdr:qwen",
                    "qwen",
                    Some("startup" | "clear" | "resume" | "compact" | "branch")
                )
                | ("herdr:antigravity_cli", "agy", None)
        )
    }

    fn session_start_source_is_recognized(session_start_source: Option<&str>) -> bool {
        matches!(
            session_start_source,
            Some("startup" | "clear" | "resume" | "compact" | "new" | "fork" | "select")
        )
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

    pub fn set_agent_name(&mut self, name: String) {
        self.agent_name = (!name.is_empty()).then_some(name);
        self.agent_name_owner = self.agent_name.as_ref().and_then(|_| {
            self.persisted_agent_session
                .as_ref()
                .map(|session| AgentNameOwner {
                    agent_label: session.agent.clone(),
                    session_ref: Some(session.session_ref.clone()),
                })
                .or_else(|| {
                    self.effective_agent_label()
                        .map(|agent_label| AgentNameOwner {
                            agent_label: agent_label.to_string(),
                            session_ref: None,
                        })
                })
        });
    }

    pub fn begin_managed_agent(
        &mut self,
        name: String,
        kind: Agent,
        now: Instant,
        settle_delay: Duration,
        timeout: Duration,
    ) {
        self.set_agent_name(name);
        self.agent_name_owner = Some(AgentNameOwner {
            agent_label: crate::detect::agent_label(kind).to_string(),
            session_ref: None,
        });
        self.managed_agent = Some(ManagedAgent {
            kind,
            phase: ManagedAgentPhase::Pending {
                ready_after: Some(now.checked_add(settle_delay).unwrap_or(now)),
                deadline: now.checked_add(timeout).unwrap_or(now),
                observed_expected: false,
            },
        });
    }

    pub fn managed_agent_launch_pending(&self) -> bool {
        self.managed_agent.is_some_and(|managed| {
            matches!(
                managed.phase,
                ManagedAgentPhase::Pending { .. } | ManagedAgentPhase::Blocked
            )
        })
    }

    pub fn managed_agent_interactive_ready(&self) -> bool {
        self.managed_agent
            .is_some_and(|managed| matches!(managed.phase, ManagedAgentPhase::Active))
    }

    pub fn managed_agent_kind(&self) -> Option<Agent> {
        self.managed_agent.map(|managed| managed.kind)
    }

    pub fn next_managed_agent_deadline(&self) -> Option<Instant> {
        let ManagedAgentPhase::Pending {
            ready_after,
            deadline,
            ..
        } = self.managed_agent?.phase
        else {
            return None;
        };
        Some(ready_after.unwrap_or(deadline).min(deadline))
    }

    pub fn reconcile_managed_agent_at(&mut self, now: Instant, process_exited: bool) -> bool {
        let Some(managed) = self.managed_agent else {
            return false;
        };
        let known_agent = self.effective_known_agent();
        let observed_expected = match managed.phase {
            ManagedAgentPhase::Pending {
                observed_expected, ..
            } => observed_expected || known_agent == Some(managed.kind),
            ManagedAgentPhase::Blocked | ManagedAgentPhase::Active => false,
        };
        let clear = process_exited
            || known_agent.is_some_and(|agent| agent != managed.kind)
            || matches!(managed.phase, ManagedAgentPhase::Pending { .. })
                && observed_expected
                && known_agent.is_none();
        if clear {
            self.clear_agent_name();
            return true;
        }
        if managed.phase == ManagedAgentPhase::Blocked {
            if known_agent == Some(managed.kind) && self.state == AgentState::Idle {
                self.managed_agent = Some(ManagedAgent {
                    kind: managed.kind,
                    phase: ManagedAgentPhase::Active,
                });
                self.managed_agent_launch_session = None;
                return true;
            }
            return false;
        }
        if let ManagedAgentPhase::Pending {
            ready_after,
            deadline,
            observed_expected: previous_observed_expected,
        } = managed.phase
        {
            if known_agent == Some(managed.kind) && self.state == AgentState::Blocked {
                self.managed_agent = Some(ManagedAgent {
                    kind: managed.kind,
                    phase: ManagedAgentPhase::Blocked,
                });
                return true;
            }
            if now >= deadline {
                self.clear_agent_name();
                return true;
            }
            if ready_after.is_none_or(|ready_after| now >= ready_after) {
                if known_agent == Some(managed.kind) && self.state == AgentState::Idle {
                    self.managed_agent = Some(ManagedAgent {
                        kind: managed.kind,
                        phase: ManagedAgentPhase::Active,
                    });
                    self.managed_agent_launch_session = None;
                    return true;
                }
                if ready_after.is_some() {
                    self.managed_agent = Some(ManagedAgent {
                        kind: managed.kind,
                        phase: ManagedAgentPhase::Pending {
                            ready_after: None,
                            deadline,
                            observed_expected,
                        },
                    });
                    return true;
                }
            }
            if observed_expected != previous_observed_expected {
                self.managed_agent = Some(ManagedAgent {
                    kind: managed.kind,
                    phase: ManagedAgentPhase::Pending {
                        ready_after,
                        deadline,
                        observed_expected,
                    },
                });
                return true;
            }
        }
        false
    }

    pub fn restore_managed_agent(&mut self, name: String, kind: Agent) {
        self.set_agent_name(name);
        self.agent_name_owner = Some(AgentNameOwner {
            agent_label: crate::detect::agent_label(kind).to_string(),
            session_ref: None,
        });
        self.managed_agent = Some(ManagedAgent {
            kind,
            phase: ManagedAgentPhase::Active,
        });
    }

    pub fn clear_agent_name(&mut self) {
        if self
            .managed_agent_launch_session
            .take()
            .as_ref()
            .is_some_and(|session| self.persisted_agent_session.as_ref() == Some(session))
        {
            self.persisted_agent_session = None;
        }
        self.agent_name = None;
        self.agent_name_owner = None;
        self.managed_agent = None;
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

    pub fn is_agent_terminal(&self) -> bool {
        self.agent_name.is_some() || self.effective_agent_label().is_some()
    }

    fn reconcile_agent_name_owner(
        &mut self,
        agent_label: &str,
        session_ref: Option<&crate::agent_resume::AgentSessionRef>,
    ) {
        if self.agent_name.is_none() {
            return;
        }
        if self.managed_agent.is_some_and(|managed| {
            crate::detect::parse_agent_label(agent_label) == Some(managed.kind)
        }) {
            return;
        }
        match self.agent_name_owner.as_mut() {
            Some(owner)
                if owner.agent_label != agent_label
                    || owner
                        .session_ref
                        .as_ref()
                        .zip(session_ref)
                        .is_some_and(|(current, incoming)| current != incoming) =>
            {
                self.agent_name = None;
                self.agent_name_owner = None;
            }
            Some(owner) if owner.session_ref.is_none() && session_ref.is_some() => {
                owner.session_ref = session_ref.cloned();
            }
            None => {
                self.agent_name_owner = Some(AgentNameOwner {
                    agent_label: agent_label.to_string(),
                    session_ref: session_ref.cloned(),
                })
            }
            _ => {}
        }
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

pub(crate) fn stabilize_agent_detection(detection: crate::detect::AgentDetection) -> AgentState {
    detection.state
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_terminal() -> TerminalState {
        TerminalState::new(TerminalId::alloc(), "/tmp".into())
    }

    fn session_id(value: &str) -> crate::agent_resume::AgentSessionRef {
        crate::agent_resume::AgentSessionRef::id(value).unwrap()
    }

    #[test]
    fn detected_state_follows_the_screen() {
        let mut terminal = test_terminal();
        let change = terminal
            .set_detected_state(Some(Agent::Claude), AgentState::Working)
            .unwrap();
        assert_eq!(change.state, AgentState::Working);
        assert_eq!(terminal.effective_agent_label(), Some("claude"));
        assert!(terminal
            .set_detected_state(Some(Agent::Claude), AgentState::Working)
            .is_none());
    }

    #[test]
    fn process_exit_clears_the_matching_persisted_session() {
        let mut terminal = test_terminal();
        terminal.set_detected_state(Some(Agent::Codex), AgentState::Idle);
        terminal.set_persisted_agent_session(crate::agent_resume::PersistedAgentSession {
            source: "herdr:codex".into(),
            agent: "codex".into(),
            session_ref: session_id("codex-1"),
        });
        terminal.set_detected_state_with_screen_signals_at(
            Some(Agent::Codex),
            AgentState::Unknown,
            false,
            true,
            Instant::now(),
        );
        assert!(terminal.persisted_agent_session.is_none());
        assert!(terminal.effective_agent_label().is_none());
    }

    #[test]
    fn process_exit_keeps_a_foreign_persisted_session() {
        let mut terminal = test_terminal();
        terminal.set_detected_state(Some(Agent::Codex), AgentState::Idle);
        terminal.set_persisted_agent_session(crate::agent_resume::PersistedAgentSession {
            source: "herdr:claude".into(),
            agent: "claude".into(),
            session_ref: session_id("claude-1"),
        });
        terminal.set_detected_state_with_screen_signals_at(
            Some(Agent::Codex),
            AgentState::Unknown,
            false,
            true,
            Instant::now(),
        );
        assert_eq!(
            terminal
                .persisted_agent_session
                .as_ref()
                .map(|session| session.agent.as_str()),
            Some("claude")
        );
    }

    #[test]
    fn claude_resume_replaces_the_persisted_session() {
        let mut terminal = test_terminal();
        terminal.set_detected_state(Some(Agent::Claude), AgentState::Idle);
        terminal.set_persisted_agent_session(crate::agent_resume::PersistedAgentSession {
            source: "herdr:claude".into(),
            agent: "claude".into(),
            session_ref: session_id("old"),
        });
        let mutation = terminal
            .set_agent_session_ref_for_session_start(
                "herdr:claude".into(),
                "claude".into(),
                Some(session_id("new")),
                Some(1),
                Some("resume".into()),
            )
            .unwrap();
        assert!(mutation.session_ref_changed);
        assert_eq!(
            terminal.persisted_agent_session.unwrap().session_ref.value,
            "new"
        );
    }

    #[test]
    fn claude_startup_does_not_replace_an_existing_session() {
        let mut terminal = test_terminal();
        terminal.set_detected_state(Some(Agent::Claude), AgentState::Idle);
        terminal.set_persisted_agent_session(crate::agent_resume::PersistedAgentSession {
            source: "herdr:claude".into(),
            agent: "claude".into(),
            session_ref: session_id("old"),
        });
        assert!(terminal
            .set_agent_session_ref_for_session_start(
                "herdr:claude".into(),
                "claude".into(),
                Some(session_id("new")),
                Some(1),
                Some("startup".into()),
            )
            .is_none());
        assert_eq!(
            terminal.persisted_agent_session.unwrap().session_ref.value,
            "old"
        );
    }

    #[test]
    fn a_stale_session_sequence_is_ignored() {
        let mut terminal = test_terminal();
        terminal.set_detected_state(Some(Agent::Pi), AgentState::Idle);
        assert!(terminal
            .set_agent_session_ref(
                "herdr:pi".into(),
                "pi".into(),
                Some(session_id("first")),
                Some(2),
            )
            .is_some());
        assert!(terminal
            .set_agent_session_ref(
                "herdr:pi".into(),
                "pi".into(),
                Some(session_id("older")),
                Some(2),
            )
            .is_none());
        assert_eq!(
            terminal.persisted_agent_session.unwrap().session_ref.value,
            "first"
        );
    }

    #[test]
    fn a_different_owner_does_not_replace_the_session_without_the_foreground_agent() {
        let mut terminal = test_terminal();
        terminal.set_detected_state(Some(Agent::Codex), AgentState::Idle);
        terminal.set_persisted_agent_session(crate::agent_resume::PersistedAgentSession {
            source: "herdr:codex".into(),
            agent: "codex".into(),
            session_ref: session_id("codex"),
        });
        assert!(terminal
            .set_agent_session_ref(
                "herdr:claude".into(),
                "claude".into(),
                Some(session_id("claude")),
                Some(1),
            )
            .is_none());
    }

    #[test]
    fn border_label_prefers_the_manual_label() {
        let mut terminal = test_terminal();
        terminal.set_detected_state(Some(Agent::Codex), AgentState::Idle);
        terminal.set_manual_label("notes".into());
        assert_eq!(terminal.border_label(true).as_deref(), Some("notes"));
    }

    #[test]
    fn managed_agent_becomes_ready_when_detection_is_idle() {
        let mut terminal = test_terminal();
        let now = Instant::now();
        terminal.begin_managed_agent(
            "builder".into(),
            Agent::Claude,
            now,
            Duration::from_millis(1),
            Duration::from_secs(30),
        );
        terminal.set_detected_state(Some(Agent::Claude), AgentState::Idle);
        assert!(terminal.reconcile_managed_agent_at(now + Duration::from_millis(2), false));
        assert!(terminal.managed_agent_interactive_ready());
    }
}
