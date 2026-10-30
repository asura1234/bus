//! A new provider must confirm startup, or retain its identity and report a blocker.
use super::{io, launch, mpsc, schema, AgentId, BusEvent, Method, ResponseResult, Worker};
use crate::messaging::{
    identity,
    model::{AgentStartup, StartupFailure, StartupRecovery},
};

pub(super) const STARTUP_TIMEOUT_MS: u64 =
    crate::protocol::api::AGENT_START_TIMEOUT.as_millis() as u64;
const SCREEN_LINES: u32 = 6;

fn startup_failure(reason: &str, evidence: &str) -> StartupFailure {
    let recovery = if evidence
        .lines()
        .any(|line| line.starts_with("> No conversation found with session ID:"))
    {
        "The provider reported a missing conversation; delete this agent, recreate it with its launch settings and system prompt, and rebrief it. Resend this message to the replacement agent."
    } else {
        "Inspect the agent's terminal and repair shell/provider setup before resuming. Resend this message after startup is confirmed."
    };
    StartupFailure {
        reason: reason.into(),
        recovery: recovery.into(),
    }
}

/// Only native restore evidence establishes a new attempt; opening a client does not.
pub(super) fn watch_native_resume(
    state: &mut super::BusState,
    agent: &super::RoomAgent,
    info: &schema::AgentInfo,
) -> Result<(), String> {
    let Some(attempt) = super::resume::native_attempt(agent, info) else {
        return Ok(());
    };
    if agent.deletion_pending
        || agent.session_binding_invalidated
        || agent.runtime_identity.launch_id.is_none()
        || agent.startup.as_ref().is_some_and(|startup| {
            startup.native_terminal_id.as_deref() == Some(info.terminal_id.as_str())
        })
    {
        return Ok(());
    }
    // Runtime identity may already point at a deferred restore's terminal before
    // native launch evidence arrives. Deduplicate by the startup's own terminal.
    // A different native terminal starts a new attempt. Retire the previous
    // reported startup episode, as explicit recovery does, so confirmation of
    // this attempt cannot leave its predecessor's error blocking a healthy agent.
    if agent
        .startup
        .as_ref()
        .is_some_and(|startup| startup.reported)
    {
        state
            .set_agent_error(agent.id, None)
            .map_err(|e| e.to_string())?;
    }
    state
        .set_agent_startup(
            agent.id,
            Some(AgentStartup {
                command: attempt.command.clone(),
                started_at_ms: io::now_ms(),
                reported: false,
                native_terminal_id: Some(info.terminal_id.clone()),
                recovery: None,
                failure: None,
            }),
        )
        .map_err(|e| e.to_string())
}

impl Worker {
    pub(super) fn watch_agent_startup(
        &mut self,
        id: AgentId,
        argv: Vec<String>,
    ) -> Result<(), String> {
        let mut state = self.state.clone();
        let started_at_ms = state
            .agent(id)
            .and_then(|agent| agent.startup.as_ref())
            .map_or_else(io::now_ms, |startup| startup.started_at_ms);
        state
            .set_agent_startup(
                id,
                Some(AgentStartup {
                    command: crate::platform::interactive_shell_command(&argv, "sh")
                        .ok_or("Cannot describe the provider launch command")?,
                    started_at_ms,
                    reported: false,
                    native_terminal_id: None,
                    recovery: None,
                    failure: None,
                }),
            )
            .map_err(|e| e.to_string())?;
        self.save(state)
    }

    pub(super) fn record_failed_launch_creation(
        &mut self,
        id: AgentId,
        argv: Vec<String>,
        reason: &str,
    ) -> Result<(), String> {
        let agent = self.state.agent(id).cloned().ok_or("Unknown agent")?;
        let diagnostics = self.startup_diagnostics(&agent);
        let recovery = if agent.runtime_identity.session_id.is_some() {
            "Repair the configured shell and provider setup, resume the saved conversation, then resend this message."
        } else {
            "Repair the configured shell and provider setup, then delete and recreate this sessionless agent. Rebrief it and resend this message."
        };
        let failure = StartupFailure {
            reason: format!("before terminal creation: {reason}"),
            recovery: recovery.into(),
        };
        let message = format!(
            "{} Agent identity retained; never-submitted queued requests closed.\nStartup diagnostics:\n{diagnostics}",
            failure.notice()
        );
        let mut state = self.state.clone();
        state
            .set_agent_startup(
                id,
                Some(AgentStartup {
                    command: crate::platform::interactive_shell_command(&argv, "sh")
                        .ok_or("Invalid launch command")?,
                    started_at_ms: io::now_ms(),
                    reported: true,
                    native_terminal_id: None,
                    recovery: None,
                    failure: Some(failure),
                }),
            )
            .map_err(|error| error.to_string())?;
        state.abandon_failed_startup_queue(id, io::now_ms());
        state
            .set_agent_error(id, Some(message.clone()))
            .map_err(|error| error.to_string())?;
        state
            .observe_status(id, super::RuntimeStatus::Unavailable, io::now_ms())
            .map_err(|error| error.to_string())?;
        self.save(state)?;
        Err(message)
    }

    pub(super) fn record_failed_agent_start(
        &mut self,
        id: AgentId,
        args: Vec<String>,
        outcome: Result<ResponseResult, crate::messaging::native::TransportError>,
    ) -> Result<(), String> {
        let error = match outcome {
            Err(error) if error.definitely_rejected => error,
            other => return self.agent_error(id, format!("Agent start outcome requires inspection of its owned terminal; startup is watched and will not be retried. {other:?}")),
        };
        let agent = self.state.agent(id).cloned().ok_or("Unknown agent")?;
        let message = format!(
            "{}\nStartup diagnostics:\n{}",
            error.message,
            self.startup_diagnostics(&agent)
        );
        let mut state = self.state.clone();
        let mut startup = state
            .agent(id)
            .and_then(|agent| agent.startup.clone())
            .ok_or("Agent startup watch is missing")?;
        startup.reported = true;
        if matches!(
            error.code.as_deref(),
            Some("agent_shell_starting" | "agent_shell_waiting")
        ) {
            // Shell questions are recoverable before any launch input.
            startup.recovery = Some(StartupRecovery::Unsubmitted { args });
            startup.failure = None;
        } else {
            startup.recovery = None;
            startup.failure = Some(startup_failure(&error.message, ""));
        }
        state
            .set_agent_startup(id, Some(startup))
            .map_err(|e| e.to_string())?;
        state.abandon_failed_startup_queue(id, io::now_ms());
        state
            .set_agent_error(id, Some(message.clone()))
            .map_err(|e| e.to_string())?;
        self.save(state)?;
        Err(message)
    }

    pub(super) fn cleanup_failed_startups(
        &mut self,
        infos: &[schema::AgentInfo],
        _events: &mpsc::Sender<BusEvent>,
        now_ms: u64,
    ) -> Result<(), String> {
        let pending: Vec<_> = self
            .state
            .agents()
            .filter_map(|agent| {
                agent
                    .startup
                    .clone()
                    .filter(|_| !agent.deletion_pending)
                    .map(|startup| (agent.clone(), startup))
            })
            .collect();
        for (agent, mut startup) in pending {
            if agent.missing_binary.is_some() {
                continue;
            }
            let name = identity::managed_name(agent.room_id, agent.id);
            let info = infos.iter().find(|info| {
                Some(info.terminal_id.as_str()) == agent.runtime_identity.terminal_id.as_deref()
                    && Some(info.pane_id.as_str()) == agent.runtime_identity.pane_id.as_deref()
                    && (info.name.as_deref() == Some(&name)
                        || super::resume::native_attempt(&agent, info).is_some())
            });
            let input_error = info.and_then(|info| info.launch_input_error.as_deref());
            let native_error = info
                .and_then(|info| super::resume::native_attempt(&agent, info))
                .and_then(|attempt| attempt.error.as_deref())
                .or(input_error);
            // Codex defers SessionStart until its first prompt. Native
            // interactive confirmation, or a provider already waiting at its
            // own startup dialog, proves it started. Preserve that help wait.
            if native_error.is_none()
                && info.is_some_and(|info| {
                    info.agent.as_deref() == Some(launch::provider_kind(agent.provider))
                        && (info.interactive_ready
                            || info.launch_pending
                                && (info.agent_status == schema::AgentStatus::Blocked
                                    || info.dialog_id.is_some()))
                })
            {
                let mut state = self.state.clone();
                state
                    .set_agent_startup(agent.id, None)
                    .map_err(|e| e.to_string())?;
                if startup.reported {
                    state
                        .set_agent_error(agent.id, None)
                        .map_err(|e| e.to_string())?;
                }
                self.save(state)?;
                continue;
            }
            let expired = now_ms.saturating_sub(startup.started_at_ms) >= STARTUP_TIMEOUT_MS;
            // The native server releases a launch on timeout or process exit.
            // A failed list request never reaches here, so silence is not failure.
            if !startup.reported && !expired && info.is_some() && native_error.is_none() {
                continue;
            }
            if !startup.reported {
                let evidence =
                    if input_error.is_some() && agent.runtime_identity.session_id.is_some() {
                        "> Conversation screen omitted".into()
                    } else {
                        self.startup_screen(agent.id)
                    };
                let diagnostics = self.startup_diagnostics(&agent);
                let reason = if let Some(reason) = native_error {
                    reason
                } else if expired {
                    "no provider session or interactive confirmation within 30 seconds"
                } else {
                    "the native provider launch ended before startup was confirmed"
                };
                let failure = startup_failure(reason, &evidence);
                let text = format!(
                    "{} Agent identity retained; never-submitted queued requests closed.\nProvider: {}\nCommand: {}\nLast screen lines:\n{evidence}\nStartup diagnostics:\n{diagnostics}",
                    failure.notice(), launch::provider_kind(agent.provider), startup.command
                );
                startup.failure = Some(failure);
                let mut state = self.state.clone();
                state
                    .set_agent_error(agent.id, Some(text))
                    .map_err(|e| e.to_string())?;
                tracing::warn!(
                    event = "bus.agent.startup_failed",
                    agent_id = agent.id.0,
                    provider = launch::provider_kind(agent.provider),
                    reason,
                    "Agent startup failed; identity retained and never-submitted queued requests closed"
                );
                startup.reported = true;
                state
                    .set_agent_startup(agent.id, Some(startup))
                    .map_err(|e| e.to_string())?;
                state.abandon_failed_startup_queue(agent.id, now_ms);
                // The persisted marker deduplicates this blocker across restarts.
                self.save(state)?;
            }
        }
        Ok(())
    }

    fn startup_screen(&mut self, id: AgentId) -> String {
        self.read_startup_screen(id)
            .unwrap_or_else(|error| format!("> Screen unavailable: {error}"))
    }

    pub(super) fn read_startup_screen(&mut self, id: AgentId) -> Result<String, String> {
        let identity = &self
            .state
            .agent(id)
            .ok_or("Unknown agent")?
            .runtime_identity;
        let pane = identity.pane_id.clone().ok_or("No launch pane")?;
        let terminal = identity.terminal_id.clone().ok_or("No launch terminal")?;
        self.verify_startup_pane(&pane, &terminal)?;
        self.read_verified_startup_screen(id)
    }

    /// Caller has just verified PaneGet against this launch's terminal identity.
    pub(super) fn read_verified_startup_screen(&mut self, id: AgentId) -> Result<String, String> {
        let identity = self
            .state
            .agent(id)
            .ok_or("Unknown agent")?
            .runtime_identity
            .clone();
        let pane = identity.pane_id.ok_or("No launch pane")?;
        let terminal = identity.terminal_id.ok_or("No launch terminal")?;
        let result = self
            .transport
            .request(Method::PaneRead(schema::PaneReadParams {
                pane_id: pane.clone(),
                source: schema::ReadSource::Recent,
                lines: Some(SCREEN_LINES),
                format: schema::ReadFormat::Text,
                strip_ansi: true,
                intent: Default::default(),
            }))
            .map_err(|e| e.message)?;
        self.verify_startup_pane(&pane, &terminal)?;
        let ResponseResult::PaneRead { read } = result else {
            return Err("Unexpected terminal read response".into());
        };
        if read.pane_id != pane {
            return Err("Launch pane changed".into());
        }
        let lines: Vec<_> = read
            .text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect();
        let evidence = lines
            .iter()
            .rev()
            .take(SCREEN_LINES as usize)
            .rev()
            .map(|line| {
                let line: String = line
                    .chars()
                    .filter(|c| !c.is_control() || *c == '\t')
                    .take(300)
                    .collect();
                format!("> {line}")
            })
            .collect::<Vec<_>>()
            .join("\n");
        Ok(if evidence.is_empty() {
            "> (screen is empty)".into()
        } else {
            evidence
        })
    }

    fn verify_startup_pane(&mut self, pane: &str, terminal: &str) -> Result<(), String> {
        match self.transport.request(Method::PaneGet(schema::PaneTarget {
            pane_id: pane.into(),
        })) {
            Ok(ResponseResult::PaneInfo { pane: info })
                if info.pane_id == pane && info.terminal_id == terminal =>
            {
                Ok(())
            }
            _ => Err("Launch terminal is no longer available at its recorded pane".into()),
        }
    }
}
