//! A new provider must confirm startup, or retain its identity and report a blocker.
use super::{io, launch, mpsc, schema, AgentId, BusEvent, Method, ResponseResult, Worker};
use crate::messaging::{identity, model::AgentStartup};

pub(super) const STARTUP_TIMEOUT_MS: u64 = 30_000;
const SCREEN_LINES: u32 = 6;

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
                    preserve_on_failure: false,
                }),
            )
            .map_err(|e| e.to_string())?;
        self.save(state)
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
                    && info.name.as_deref() == Some(&name)
            });
            // Codex defers SessionStart until its first prompt. Native
            // interactive confirmation, or a provider already waiting at its
            // own startup dialog, proves it started. Preserve that help wait.
            if info.is_some_and(|info| {
                info.agent.as_deref() == Some(launch::provider_kind(agent.provider))
                    && (info.interactive_ready
                        || info.launch_pending
                            && (info.agent_status == schema::AgentStatus::Blocked
                                || info.dialog_id.is_some()))
            }) {
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
            if !startup.reported && !expired && info.is_some() {
                continue;
            }
            if !startup.reported {
                let evidence = self.startup_screen(agent.id);
                let reason = if expired {
                    "no provider session or interactive confirmation within 30 seconds"
                } else {
                    "the native provider launch ended before startup was confirmed"
                };
                let text = format!(
                    "Startup failed: {reason}. Agent identity and queued work retained. Inspect readiness or resume this agent.\nProvider: {}\nCommand: {}\nLast screen lines:\n{evidence}",
                    launch::provider_kind(agent.provider), startup.command
                );
                let mut state = self.state.clone();
                state
                    .set_agent_error(agent.id, Some(text))
                    .map_err(|e| e.to_string())?;
                tracing::warn!(
                    event = "bus.agent.startup_failed",
                    agent_id = agent.id.0,
                    provider = launch::provider_kind(agent.provider),
                    reason,
                    "Agent startup failed; identity and queued work retained"
                );
                startup.reported = true;
                state
                    .set_agent_startup(agent.id, Some(startup))
                    .map_err(|e| e.to_string())?;
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
