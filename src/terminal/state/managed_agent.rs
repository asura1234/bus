use std::time::{Duration, Instant};

use super::{AgentNameOwner, ManagedAgent, ManagedAgentPhase, TerminalState};
use crate::agents::{AgentKind, AgentState};

impl TerminalState {
    pub(crate) fn bind_managed_launch_pending(
        &mut self,
        signal: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        if let Some(previous) = self.managed_launch_pending_signal.replace(signal) {
            previous.store(false, std::sync::atomic::Ordering::Release);
        }
        self.sync_managed_launch_pending();
    }

    fn update_managed_agent(&mut self, managed: Option<ManagedAgent>) {
        if self.pending_agent_resume_plan.is_none()
            && managed.is_some_and(|managed| {
                matches!(
                    managed.phase,
                    ManagedAgentPhase::Active | ManagedAgentPhase::Blocked
                )
            })
        {
            self.native_resume = None;
        }
        self.managed_agent = managed;
        self.sync_managed_launch_pending();
    }

    fn sync_managed_launch_pending(&self) {
        if let Some(signal) = &self.managed_launch_pending_signal {
            signal.store(
                self.managed_agent_launch_pending(),
                std::sync::atomic::Ordering::Release,
            );
        }
    }

    pub fn set_managed_agent_launch_session(
        &mut self,
        session: crate::agents::resume::catalog::PersistedAgentSession,
    ) {
        self.persisted_agent_session = Some(session.clone());
        self.session_reporter.clear();
        self.managed_agent_launch_session = Some(session);
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
        kind: AgentKind,
        now: Instant,
        settle_delay: Duration,
        timeout: Duration,
    ) {
        // Retire the old exit together with its stale detected provider label.
        if self.recent_agent_process_exit.take().is_some() {
            self.detected_agent = None;
        }
        self.set_agent_name(name);
        self.agent_name_owner = Some(AgentNameOwner {
            agent_label: crate::agents::agent_label(kind).to_string(),
            session_ref: None,
        });
        self.update_managed_agent(Some(ManagedAgent {
            kind,
            phase: ManagedAgentPhase::Pending {
                ready_after: Some(now.checked_add(settle_delay).unwrap_or(now)),
                deadline: now.checked_add(timeout).unwrap_or(now),
                observed_expected: false,
            },
        }));
    }

    pub fn managed_agent_launch_pending(&self) -> bool {
        self.pending_agent_resume_plan.is_some()
            || self.managed_agent_identity_unestablished()
            || self
                .managed_agent
                .is_some_and(|managed| managed.phase == ManagedAgentPhase::TimedOut)
    }

    /// Deferred restores and timed-out launches retain owned identity in
    /// snapshots even without readiness. Only a new pending launch is omitted.
    pub fn managed_agent_identity_unestablished(&self) -> bool {
        self.managed_agent.is_some_and(|managed| {
            matches!(
                managed.phase,
                ManagedAgentPhase::Pending { .. } | ManagedAgentPhase::Blocked
            )
        })
    }

    pub fn managed_agent_interactive_ready(&self) -> bool {
        self.pending_agent_resume_plan.is_none()
            && self
                .managed_agent
                .is_some_and(|managed| matches!(managed.phase, ManagedAgentPhase::Active))
    }

    pub(super) fn confirm_managed_agent_session_start(&mut self, kind: AgentKind) {
        if self.managed_agent_launch_pending() && self.managed_agent_kind() == Some(kind) {
            self.update_managed_agent(Some(ManagedAgent {
                kind,
                phase: ManagedAgentPhase::Active,
            }));
            self.managed_agent_launch_session = None;
        }
    }

    pub fn managed_agent_kind(&self) -> Option<AgentKind> {
        self.managed_agent.map(|managed| managed.kind)
    }

    pub(crate) fn begin_native_resume(&mut self, command: String) {
        if let (Some(name), Some(kind)) = (self.agent_name.clone(), self.managed_agent_kind()) {
            self.native_resume = Some(super::NativeAgentResume {
                name,
                kind,
                session: self.persisted_agent_session.clone(),
                command,
                error: None,
            });
        }
    }

    pub(crate) fn suspend_native_resume(&mut self, reason: &str) {
        if let Some(resume) = self.native_resume.as_mut() {
            resume.error = Some(reason.into());
        }
        if let Some(managed) = self.managed_agent {
            self.update_managed_agent(Some(ManagedAgent {
                kind: managed.kind,
                phase: ManagedAgentPhase::Suspended,
            }));
        }
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
            ManagedAgentPhase::Blocked
            | ManagedAgentPhase::TimedOut
            | ManagedAgentPhase::Active
            | ManagedAgentPhase::Suspended => false,
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
        if matches!(
            managed.phase,
            ManagedAgentPhase::Blocked | ManagedAgentPhase::TimedOut
        ) {
            if known_agent == Some(managed.kind) && self.state == AgentState::Idle {
                self.update_managed_agent(Some(ManagedAgent {
                    kind: managed.kind,
                    phase: ManagedAgentPhase::Active,
                }));
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
                self.update_managed_agent(Some(ManagedAgent {
                    kind: managed.kind,
                    phase: ManagedAgentPhase::Blocked,
                }));
                return true;
            }
            if now >= deadline {
                // Expiry is not a terminal replacement. Keep ownership and
                // startup detection until positive readiness or a real exit,
                // but consume the deadline so the scheduler cannot spin.
                if let Some(resume) = self.native_resume.as_mut() {
                    resume.error = Some("provider startup confirmation deadline elapsed".into());
                }
                self.update_managed_agent(Some(ManagedAgent {
                    kind: managed.kind,
                    phase: ManagedAgentPhase::TimedOut,
                }));
                return true;
            }
            if ready_after.is_none_or(|ready_after| now >= ready_after) {
                if known_agent == Some(managed.kind) && self.state == AgentState::Idle {
                    self.update_managed_agent(Some(ManagedAgent {
                        kind: managed.kind,
                        phase: ManagedAgentPhase::Active,
                    }));
                    self.managed_agent_launch_session = None;
                    return true;
                }
                if ready_after.is_some() {
                    self.update_managed_agent(Some(ManagedAgent {
                        kind: managed.kind,
                        phase: ManagedAgentPhase::Pending {
                            ready_after: None,
                            deadline,
                            observed_expected,
                        },
                    }));
                    return true;
                }
            }
            if observed_expected != previous_observed_expected {
                self.update_managed_agent(Some(ManagedAgent {
                    kind: managed.kind,
                    phase: ManagedAgentPhase::Pending {
                        ready_after,
                        deadline,
                        observed_expected,
                    },
                }));
                return true;
            }
        }
        false
    }

    pub fn restore_managed_agent(&mut self, name: String, kind: AgentKind) {
        self.set_agent_name(name);
        self.agent_name_owner = Some(AgentNameOwner {
            agent_label: crate::agents::agent_label(kind).to_string(),
            session_ref: None,
        });
        self.update_managed_agent(Some(ManagedAgent {
            kind,
            phase: ManagedAgentPhase::Active,
        }));
    }

    pub fn clear_agent_name(&mut self) {
        if let Some(resume) = self.native_resume.as_mut() {
            resume
                .error
                .get_or_insert_with(|| "provider launch ended before startup was confirmed".into());
        }
        if self
            .managed_agent_launch_session
            .take()
            .as_ref()
            .is_some_and(|session| self.persisted_agent_session.as_ref() == Some(session))
        {
            self.persisted_agent_session = None;
            self.session_reporter.clear();
        }
        self.agent_name = None;
        self.agent_name_owner = None;
        self.update_managed_agent(None);
    }

    pub(super) fn reconcile_agent_name_owner(
        &mut self,
        agent_label: &str,
        session_ref: Option<&crate::agents::resume::catalog::AgentSessionRef>,
    ) {
        if self.agent_name.is_none() {
            return;
        }
        if self.managed_agent.is_some_and(|managed| {
            crate::agents::parse_agent_label(agent_label) == Some(managed.kind)
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
}
