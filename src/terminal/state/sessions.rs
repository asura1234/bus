use std::time::Instant;

use super::{EffectivePresentation, TerminalState, TerminalStateMutation};

impl TerminalState {
    pub fn set_persisted_agent_session(
        &mut self,
        session: crate::agent_resume::PersistedAgentSession,
    ) {
        self.persisted_agent_session = Some(session);
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

    pub(super) fn current_session_identity_for_persistence(
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
}
