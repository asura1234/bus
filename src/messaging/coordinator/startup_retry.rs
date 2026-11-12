//! Retry only a proven sessionless Codex bootstrap exit, never provider input.
use super::{schema, AgentRuntimeIdentity, Method, Provider, ResponseResult, RoomAgent, Worker};
use crate::messaging::{
    identity,
    model::{AgentStartup, RequestPhase, RuntimeStatus, StartupRecovery},
};

fn transient(screen: &str) -> bool {
    let screen = screen
        .lines()
        .map(|line| line.trim().trim_start_matches("> "))
        .collect::<Vec<_>>()
        .join(" ");
    screen.contains("account/read failed")
        && (screen.contains("workspace routing discovery failed")
            || screen.contains("workspace routing discovery timed out"))
        && !screen.contains("unauthorized")
        && !screen.contains("401")
        && !screen.contains("requires a ChatGPT account id")
        && !screen.contains("configuration changed")
}

impl Worker {
    /// An owned pane's native exit flag covers launches that disappeared before
    /// the coordinator ever observed their process. Mere list absence does not.
    pub(super) fn startup_process_exited(&mut self, agent: &RoomAgent) -> bool {
        let AgentRuntimeIdentity {
            pane_id: Some(pane),
            terminal_id: Some(terminal),
            ..
        } = &agent.runtime_identity
        else {
            return false;
        };
        matches!(self.transport.request(Method::PaneGet(schema::PaneTarget { pane_id: pane.clone() })),
            Ok(ResponseResult::PaneInfo { pane: info }) if info.pane_id == *pane
                && info.terminal_id == *terminal && info.agent_process_exited)
    }

    pub(super) fn capture_startup_exit(&mut self, agent: &RoomAgent) -> Result<(), String> {
        let AgentRuntimeIdentity {
            pane_id: Some(pane),
            terminal_id: Some(terminal),
            ..
        } = &agent.runtime_identity
        else {
            return Ok(());
        };
        // Identity is checked on both sides of the read just like startup diagnostics.
        if self.read_startup_screen(agent.id).is_err() {
            return Ok(());
        }
        let mut state = self.state.clone();
        super::exit_snapshot::save(
            self.transport.as_mut(),
            &self.data_dir,
            &mut state,
            agent,
            pane,
            terminal,
        )?;
        if self.read_startup_screen(agent.id).is_err() {
            return Ok(());
        }
        self.save(state)
    }

    /// true means this episode is held by the retry state, so cleanup must not
    /// abandon its queue. Backoff and consumed input authority survive restart.
    pub(super) fn handle_codex_startup_retry(
        &mut self,
        agent: &RoomAgent,
        startup: &mut AgentStartup,
        process_exited: bool,
        now: u64,
    ) -> Result<bool, String> {
        let Some(StartupRecovery::CodexBootstrap {
            args,
            attempts,
            retry_at_ms,
            uncertain,
            exhausted,
        }) = startup.recovery.clone()
        else {
            return Ok(false);
        };
        if startup.reported || exhausted {
            return Ok(false);
        }
        if uncertain {
            return Ok(true);
        }
        if agent.provider != Provider::Codex
            || agent.session_binding_invalidated
            || agent.runtime_identity.session_id.is_some()
            || agent.current_request.is_some()
            || self.native_submissions.contains_key(&agent.id)
            || self.state.requests().any(|r| {
                r.agent_id == agent.id
                    && r.phase != RequestPhase::Queued
                    && !matches!(
                        r.failure_reason.as_deref(),
                        Some(
                            crate::messaging::model::AGENT_STARTUP_FAILED
                                | crate::messaging::model::CODEX_BOOTSTRAP_FAILED
                        )
                    )
            })
        {
            return Ok(false);
        }
        if let Some(due) = retry_at_ms {
            if now < due {
                return Ok(true);
            }
            if !process_exited {
                // A live/replaced/unreadable pane never authorizes a second launch.
                return Ok(false);
            }
            self.retry_codex_bootstrap(agent, startup, args, attempts + 1, now)?;
            return Ok(true);
        }
        if !process_exited {
            return Ok(false);
        }
        self.capture_startup_exit(agent)?;
        if self
            .state
            .agent(agent.id)
            .and_then(|a| a.exit_snapshot.as_ref())
            .is_none_or(|s| s.path.is_none())
        {
            return Ok(false);
        }
        let screen = self.read_startup_screen(agent.id).unwrap_or_default();
        if !transient(&screen) {
            return Ok(false);
        }
        if attempts >= 2 {
            startup.recovery = Some(StartupRecovery::CodexBootstrap {
                args,
                attempts,
                retry_at_ms: None,
                uncertain: false,
                exhausted: true,
            });
            return Ok(false);
        }
        let due = now.saturating_add(if attempts == 0 { 5_000 } else { 20_000 });
        startup.recovery = Some(StartupRecovery::CodexBootstrap {
            args,
            attempts,
            retry_at_ms: Some(due),
            uncertain: false,
            exhausted: false,
        });
        let mut state = self.state.clone();
        state
            .set_agent_startup(agent.id, Some(startup.clone()))
            .map_err(|e| e.to_string())?;
        state.watch_launch_command(agent.id, due);
        state
            .observe_status(agent.id, RuntimeStatus::Launching, now)
            .map_err(|e| e.to_string())?;
        self.save(state)?;
        Ok(true)
    }

    fn retry_codex_bootstrap(
        &mut self,
        agent: &RoomAgent,
        startup: &mut AgentStartup,
        args: Vec<String>,
        attempts: u32,
        now: u64,
    ) -> Result<(), String> {
        let pane = agent
            .runtime_identity
            .pane_id
            .clone()
            .ok_or("No launch pane")?;
        let terminal = agent
            .runtime_identity
            .terminal_id
            .clone()
            .ok_or("No launch terminal")?;
        startup.recovery = Some(StartupRecovery::CodexBootstrap {
            args: args.clone(),
            attempts,
            retry_at_ms: None,
            uncertain: true,
            exhausted: false,
        });
        startup.started_at_ms = now;
        startup.process_observed = false;
        let mut state = self.state.clone();
        state
            .set_agent_startup(agent.id, Some(startup.clone()))
            .map_err(|e| e.to_string())?;
        state.watch_launch_command(agent.id, now);
        self.save(state)?;
        tracing::info!(
            event = "bus.agent.startup_retry",
            agent_id = agent.id.0,
            provider = "codex",
            attempt = attempts,
            "Retrying exited Codex bootstrap; queue retained"
        );
        let name = identity::managed_name(agent.room_id, agent.id);
        let result = self.transport.request(Method::AgentStartIfIdentity(
            schema::AgentStartIfIdentityParams {
                expected_terminal_id: terminal.clone(),
                start: schema::AgentStartParams {
                    name: name.clone(),
                    kind: "codex".into(),
                    pane_id: pane.clone(),
                    args: args.clone(),
                    executable: None,
                    grok_home: None,
                    kimi_home: None,
                    timeout_ms: None,
                },
            },
        ));
        match result {
            Ok(ResponseResult::AgentStarted { agent: info, .. })
                if info.terminal_id == terminal
                    && info.pane_id == pane
                    && info.name.as_deref() == Some(&name) =>
            {
                startup.recovery = Some(StartupRecovery::CodexBootstrap {
                    args,
                    attempts,
                    retry_at_ms: None,
                    uncertain: false,
                    exhausted: false,
                });
                let mut state = self.state.clone();
                state
                    .set_agent_startup(agent.id, Some(startup.clone()))
                    .map_err(|e| e.to_string())?;
                self.save(state)
            }
            other => self.record_failed_agent_start(agent.id, args, other),
        }
    }
}
