//! A new provider must confirm startup, or be reported and deleted. This
//! watches the existing native launch outcome; it never types or retries.
use super::{dialogs, io, launch, mpsc, schema, AgentId, BusEvent, Method, ResponseResult, Worker};
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
                }),
            )
            .map_err(|e| e.to_string())?;
        self.save(state)
    }

    pub(super) fn cleanup_failed_startups(
        &mut self,
        infos: &[schema::AgentInfo],
        events: &mpsc::Sender<BusEvent>,
        now_ms: u64,
    ) -> Result<(), String> {
        let pending: Vec<_> = self
            .state
            .agents()
            .filter_map(|agent| {
                agent
                    .startup
                    .clone()
                    .filter(|startup| !agent.deletion_pending || startup.reported)
                    .map(|startup| (agent.clone(), startup))
            })
            .collect();
        for (agent, mut startup) in pending {
            let name = identity::managed_name(agent.room_id, agent.id);
            let info = infos.iter().find(|info| {
                Some(info.terminal_id.as_str()) == agent.runtime_identity.terminal_id.as_deref()
                    && Some(info.pane_id.as_str()) == agent.runtime_identity.pane_id.as_deref()
                    && info.name.as_deref() == Some(&name)
            });
            // Codex defers SessionStart until its first prompt. Native
            // interactive confirmation is the existing equivalent start signal.
            if !startup.reported
                && info.is_some_and(|info| {
                    info.interactive_ready
                        && info.agent.as_deref() == Some(launch::provider_kind(agent.provider))
                })
            {
                let mut state = self.state.clone();
                state
                    .set_agent_startup(agent.id, None)
                    .map_err(|e| e.to_string())?;
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
                    "{} (agent {}). Startup failed: {reason}. Removing this agent from the room.\nProvider: {}\nCommand: {}\nLast screen lines:\n{evidence}",
                    agent.name, agent.id.0, launch::provider_kind(agent.provider), startup.command
                );
                let mut state = self.state.clone();
                dialogs::post_dialog_notice(&mut state, agent.id, true, text)?;
                startup.reported = true;
                state
                    .set_agent_startup(agent.id, Some(startup))
                    .map_err(|e| e.to_string())?;
                // Reporting is durable before normal guarded deletion begins.
                self.save(state)?;
            }
            self.delete_agent(agent.id, events)?;
        }
        Ok(())
    }

    fn startup_screen(&mut self, id: AgentId) -> String {
        self.read_startup_screen(id)
            .unwrap_or_else(|error| format!("> Screen unavailable: {error}"))
    }

    fn read_startup_screen(&mut self, id: AgentId) -> Result<String, String> {
        let identity = self
            .state
            .agent(id)
            .ok_or("Unknown agent")?
            .runtime_identity
            .clone();
        let pane = identity.pane_id.ok_or("No launch pane")?;
        let terminal = identity.terminal_id.ok_or("No launch terminal")?;
        self.verify_startup_pane(&pane, &terminal)?;
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
