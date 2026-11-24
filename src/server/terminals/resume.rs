use std::time::Instant;

use ratatui::layout::Rect;

use crate::agents::resume::catalog::{AgentResumePlan, PersistedAgentSession};
use crate::messaging::coordinator::resume::{
    self, LaunchExtras, NativeResumeContext, NativeResumeFacts,
};
use crate::server::app::App;
use crate::utils::ids::{PaneId, TerminalId};
use tokio::sync::oneshot;

type ResumeInputs = (Vec<String>, Vec<(String, String)>);
// Four panes keep restart responsive without a burst of provider probes and startups.
const MAX_CONCURRENT_AGENT_RESUMES: usize = 4;

fn terminal_resume_facts(terminal: &crate::terminal::TerminalState) -> NativeResumeFacts {
    NativeResumeFacts {
        agent_name: terminal.agent_name.clone(),
        managed_agent: terminal
            .managed_agent_kind()
            .map(|kind| managed_resume_label(kind, terminal.launch_argv.as_deref()))
            .map(str::to_owned),
        session: terminal.persisted_agent_session.clone(),
    }
}

fn managed_resume_label(kind: crate::agents::AgentKind, argv: Option<&[String]>) -> &'static str {
    // The Claude process has a shared native kind. The recorded launch argv
    // distinguishes the wrapper until the room and callback owner are verified.
    if kind == crate::agents::AgentKind::Claude {
        let executable = argv
            .and_then(|argv| argv.first())
            .and_then(|command| std::path::Path::new(command).file_name());
        for provider in [
            crate::agents::providers::ProviderKind::ClaudeArgon,
            crate::agents::providers::ProviderKind::ClaudeGpt,
        ] {
            if executable.is_some_and(|name| name == provider.executable()) {
                return provider.label();
            }
        }
    }
    crate::agents::agent_label(kind)
}

pub(crate) fn unstarted_restore_plan(
    agent_name: Option<&str>,
    managed_agent: Option<crate::agents::AgentKind>,
    argv: Option<&[String]>,
) -> Option<crate::agents::resume::catalog::AgentResumePlan> {
    let facts = NativeResumeFacts {
        agent_name: agent_name.map(str::to_owned),
        managed_agent: managed_agent
            .map(|kind| managed_resume_label(kind, argv))
            .map(str::to_owned),
        session: None,
    };
    crate::messaging::coordinator::resume::unstarted_plan(&facts, argv)
}

fn native_resume_context(
    facts: &NativeResumeFacts,
    cwd: &std::path::Path,
) -> Result<Option<NativeResumeContext>, String> {
    if !facts.is_bus_owned() {
        return Ok(None);
    }
    // Capture the same env bytes locally, without importing the CLI edge.
    let root = std::env::var_os("BUS_DATA_DIR").map(std::path::PathBuf::from);
    let session_name = crate::utils::paths::active_name();
    let config_root = crate::utils::config::config_dir();
    if !root
        .as_ref()
        .is_some_and(|root| root.is_absolute() && config_root == root.join("herdr-config"))
        || session_name.as_deref() != Some(crate::messaging::coordinator::DEFAULT_SESSION)
    {
        return Ok(None);
    }
    let binary = crate::platform::fs::running_executable()
        .map_err(|_| "Bus resume executable unavailable")?;
    Ok(Some(NativeResumeContext {
        root,
        session_name,
        config_root,
        project: cwd.to_path_buf(),
        binary,
    }))
}

fn prepare_native_resume(
    mut context: NativeResumeContext,
    facts: &NativeResumeFacts,
    plan: &AgentResumePlan,
) -> Result<PreparedResume, String> {
    if let Some(project) = std::process::Command::new("git")
        .arg("-C")
        .arg(&context.project)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| std::path::PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()))
    {
        context.project = project;
    }
    PreparedResume::new(plan, resume::for_native_resume(&context, facts, plan)?)
}

/// Map the agents-domain kind into the API's neutral serialized DTO.
pub(crate) fn api_session_kind(
    kind: crate::agents::resume::catalog::AgentSessionRefKind,
) -> crate::protocol::api::schema::AgentSessionRefKind {
    match kind {
        crate::agents::resume::catalog::AgentSessionRefKind::Id => {
            crate::protocol::api::schema::AgentSessionRefKind::Id
        }
        crate::agents::resume::catalog::AgentSessionRefKind::Path => {
            crate::protocol::api::schema::AgentSessionRefKind::Path
        }
    }
}

struct PendingAgentResumeCandidate {
    pane_id: PaneId,
    terminal_id: TerminalId,
    cwd: std::path::PathBuf,
    plan: crate::agents::resume::catalog::AgentResumePlan,
    rows: u16,
    cols: u16,
}

pub(in crate::server) enum AgentResumeInFlight {
    Preparing(Box<PreparingResume>),
    Launching,
}

pub(in crate::server) struct PreparingResume {
    candidate: PendingAgentResumeCandidate,
    facts: NativeResumeFacts,
    context: NativeResumeContext,
    result: oneshot::Receiver<Result<PreparedResume, String>>,
}

struct PreparedResume {
    session: Option<PersistedAgentSession>,
    inputs: ResumeInputs,
}

impl PreparedResume {
    fn new(plan: &AgentResumePlan, extras: LaunchExtras) -> Result<Self, String> {
        let session = extras.session.clone();
        let corrected_plan = session
            .as_ref()
            .map(|session| {
                crate::agents::resume::catalog::plan(
                    &session.source,
                    &session.agent,
                    &session.session_ref,
                )
                .ok_or("Corrected conversation has no provider resume plan")
            })
            .transpose()?;
        let plan = corrected_plan.as_ref().unwrap_or(plan);
        if let Some(provider) = crate::agents::providers::launch::native_provider(&plan.agent) {
            crate::agents::providers::launch::preflight_gateway(provider)?;
        }
        let provider_argv = matches!(plan.agent.as_str(), "grok" | "kimi")
            .then(|| resume_argv(plan, &extras))
            .transpose()?;
        let (mut argv, env) = resume_launch_inputs(plan, extras);
        if plan.agent == "claude-gpt" {
            argv[0] = crate::agents::providers::launch::selected_executable(
                crate::agents::providers::ProviderKind::ClaudeGpt,
                &Default::default(),
            )?
            .ok_or("Claude GPT resume has no selected executable")?;
        }
        Ok(Self {
            session,
            inputs: (provider_argv.unwrap_or(argv), env),
        })
    }
}

impl App {
    pub(crate) fn has_pending_agent_resumes(&self) -> bool {
        self.state
            .terminals
            .values()
            .any(|terminal| terminal.pending_agent_resume_plan.is_some())
    }

    pub(crate) fn sync_pending_agent_resume_deadline(&mut self, now: Instant) {
        if !self.has_pending_agent_resumes() {
            self.pending_agent_resume_deadline = None;
            return;
        }
        if self.pending_agent_resume_candidates().is_empty() {
            self.pending_agent_resume_deadline = None;
            return;
        }
        self.pending_agent_resume_deadline
            .get_or_insert(now + crate::server::app::PENDING_AGENT_RESUME_THEME_WAIT);
    }

    pub(crate) fn pending_agent_resume_due(&self, now: Instant) -> bool {
        self.pending_agent_resume_deadline
            .is_some_and(|deadline| now >= deadline)
    }

    pub(crate) fn start_pending_agent_resumes(&mut self, allow_empty_theme: bool) -> bool {
        self.agent_resumes_in_flight.retain(|id, phase| {
            matches!(phase, AgentResumeInFlight::Preparing(_))
                || self.terminal_runtimes.get(id).is_some_and(|runtime| {
                    runtime.launch_input_pending()
                        || (runtime.definite_launch_input_error().is_none()
                            && self.state.terminals.get(id).is_some_and(|terminal| {
                                terminal.next_managed_agent_deadline().is_some()
                            }))
                })
        });
        let mut changed = self.finish_prepared_agent_resumes();
        if !self.agent_resume_at_capacity()
            && self.has_pending_agent_resumes()
            && (!self.state.host_terminal_theme.is_empty() || allow_empty_theme)
        {
            for candidate in self.pending_agent_resume_candidates() {
                if self.agent_resume_at_capacity() {
                    break;
                }
                if self
                    .agent_resumes_in_flight
                    .contains_key(&candidate.terminal_id)
                    || self.terminal_runtimes.get(&candidate.terminal_id).is_some()
                {
                    continue;
                }
                changed |= self.prepare_pending_agent_resume(candidate);
            }
        }

        if changed {
            self.schedule_session_save();
        }
        if !self.has_pending_agent_resumes() || self.pending_agent_resume_candidates().is_empty() {
            self.pending_agent_resume_deadline = None;
        }
        changed
    }

    pub(in crate::server) fn agent_resume_at_capacity(&self) -> bool {
        self.agent_resumes_in_flight.len() >= MAX_CONCURRENT_AGENT_RESUMES
    }

    fn prepare_pending_agent_resume(&mut self, candidate: PendingAgentResumeCandidate) -> bool {
        let facts = terminal_resume_facts(&self.state.terminals[&candidate.terminal_id]);
        match native_resume_context(&facts, &candidate.cwd) {
            Ok(Some(context)) => {
                let (sender, result) = oneshot::channel();
                let worker_context = context.clone();
                let worker_facts = facts.clone();
                let plan = candidate.plan.clone();
                let wake = self.render_notify.clone();
                tokio::task::spawn_blocking(move || {
                    let _ =
                        sender.send(prepare_native_resume(worker_context, &worker_facts, &plan));
                    wake.notify_one();
                });
                self.agent_resumes_in_flight.insert(
                    candidate.terminal_id.clone(),
                    AgentResumeInFlight::Preparing(Box::new(PreparingResume {
                        candidate,
                        facts,
                        context,
                        result,
                    })),
                );
                false
            }
            Ok(None) => {
                let prepared = PreparedResume::new(&candidate.plan, LaunchExtras::default());
                self.start_prepared_agent_resume(candidate, prepared)
            }
            Err(reason) => self.start_prepared_agent_resume(candidate, Err(reason)),
        }
    }

    fn finish_prepared_agent_resumes(&mut self) -> bool {
        let finished: Vec<_> = self
            .agent_resumes_in_flight
            .iter_mut()
            .filter_map(|(id, phase)| {
                let AgentResumeInFlight::Preparing(job) = phase else {
                    return None;
                };
                let result = match job.result.try_recv() {
                    Ok(result) => result,
                    Err(oneshot::error::TryRecvError::Closed) => {
                        Err("Provider resume preparation stopped".into())
                    }
                    Err(oneshot::error::TryRecvError::Empty) => return None,
                };
                Some((id.clone(), result))
            })
            .collect();
        let mut changed = false;
        for (id, result) in finished {
            let Some(AgentResumeInFlight::Preparing(job)) =
                self.agent_resumes_in_flight.remove(&id)
            else {
                continue;
            };
            let PreparingResume {
                candidate,
                facts,
                context,
                ..
            } = *job;
            // The loop still owns every PTY and applies the result only to the
            // exact pane, plan and saved identity that the worker prepared.
            if self.terminal_runtimes.get(&id).is_some()
                || !self
                    .find_pane(candidate.pane_id)
                    .is_some_and(|(_, pane)| pane.attached_terminal_id == id)
                || !self.state.terminals.get(&id).is_some_and(|terminal| {
                    terminal.pending_agent_resume_plan.as_ref() == Some(&candidate.plan)
                        && terminal.cwd == candidate.cwd
                        && terminal_resume_facts(terminal) == facts
                })
            {
                continue;
            }
            let result = result.and_then(|prepared| {
                resume::revalidate(
                    &context,
                    &facts,
                    &candidate.plan,
                    prepared.session.as_ref(),
                    &prepared.inputs.1,
                )?;
                Ok(prepared)
            });
            changed |= self.start_prepared_agent_resume(candidate, result);
        }
        changed
    }

    fn pending_agent_resume_candidates(&self) -> Vec<PendingAgentResumeCandidate> {
        let terminal_area = self.state.view.terminal_area;
        if terminal_area.width == 0 || terminal_area.height == 0 {
            return Vec::new();
        };

        let mut pending = Vec::new();
        for (ws_idx, ws) in self.state.workspaces.iter().enumerate() {
            for (tab_idx, tab) in ws.tabs.iter().enumerate() {
                for info in
                    self.pending_agent_resume_pane_infos(ws_idx, tab_idx, tab, terminal_area)
                {
                    let Some(pane) = tab.panes.get(&info.id) else {
                        continue;
                    };
                    if self
                        .terminal_runtimes
                        .get(&pane.attached_terminal_id)
                        .is_some()
                    {
                        continue;
                    }
                    let Some(terminal) = self.state.terminals.get(&pane.attached_terminal_id)
                    else {
                        continue;
                    };
                    let Some(plan) = terminal.pending_agent_resume_plan.clone() else {
                        continue;
                    };
                    pending.push(PendingAgentResumeCandidate {
                        pane_id: info.id,
                        terminal_id: pane.attached_terminal_id.clone(),
                        cwd: terminal.cwd.clone(),
                        plan,
                        rows: info.inner_rect.height,
                        cols: info.inner_rect.width,
                    });
                }
            }
        }
        pending
    }

    fn pending_agent_resume_pane_infos(
        &self,
        ws_idx: usize,
        tab_idx: usize,
        tab: &crate::server::workspaces::Tab,
        terminal_area: Rect,
    ) -> Vec<crate::server::workspaces::layout::PaneInfo> {
        #[cfg(test)]
        self.resume_geometry_derivations
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut pane_infos = derived_pending_agent_resume_pane_infos(
            tab,
            terminal_area,
            self.state.pane_borders,
            self.state.pane_gaps,
            self.state.pane_outer_borders,
        );

        if self.state.active == Some(ws_idx)
            && self
                .state
                .workspaces
                .get(ws_idx)
                .is_some_and(|ws| tab_idx == ws.active_tab_index())
        {
            for visible_info in &self.state.view.pane_infos {
                if let Some(info) = pane_infos
                    .iter_mut()
                    .find(|info| info.id == visible_info.id)
                {
                    *info = visible_info.clone();
                } else {
                    pane_infos.push(visible_info.clone());
                }
            }
        }

        pane_infos
    }

    fn record_resume_command(
        &mut self,
        terminal_id: &crate::utils::ids::TerminalId,
        argv: &[String],
    ) -> String {
        let command = shell_command_from_argv(argv);
        if let Some(terminal) = self.state.terminals.get_mut(terminal_id) {
            terminal.begin_native_resume(command.clone());
        }
        command
    }

    fn start_prepared_agent_resume(
        &mut self,
        candidate: PendingAgentResumeCandidate,
        prepared: Result<PreparedResume, String>,
    ) -> bool {
        let PendingAgentResumeCandidate {
            pane_id,
            terminal_id,
            cwd,
            plan,
            rows,
            cols,
        } = candidate;
        let host_terminal_theme = self.state.host_terminal_theme;
        let Some(terminal) = self.state.terminals.get_mut(&terminal_id) else {
            return false;
        };
        terminal.begin_native_resume(shell_command_from_argv(&plan.argv));
        let PreparedResume {
            session,
            inputs: (argv, env),
        } = match prepared {
            Ok(prepared) => prepared,
            Err(reason) => {
                return self.suspend_pending_agent_resume(pane_id, &terminal_id, &reason);
            }
        };
        if let Some(session) = session {
            tracing::info!(event = "bus.resume.session_corrected", pane = pane_id.raw(),
                terminal = %terminal_id, session = %session.session_ref.value,
                "Resuming the conversation Bus bound instead of the terminal's stale one");
            terminal.set_persisted_agent_session(session);
        }
        self.record_resume_command(&terminal_id, &argv);
        let Some(launch_env) = self
            .find_pane(pane_id)
            .and_then(|(ws_idx, _)| self.pane_launch_env(ws_idx, pane_id, env))
        else {
            return self.suspend_pending_agent_resume(
                pane_id,
                &terminal_id,
                "resume pane identity unavailable",
            );
        };

        let runtime = match crate::terminal::TerminalRuntime::spawn(
            pane_id,
            rows,
            cols,
            cwd,
            self.state.pane_scrollback_limit_bytes,
            host_terminal_theme,
            self.state.host_terminal_appearance,
            crate::terminal::runtime::PaneShellConfig::new(
                &self.state.default_shell,
                self.state.shell_mode,
            ),
            &launch_env,
            self.event_tx.clone(),
            self.render_notify.clone(),
            self.render_dirty.clone(),
        ) {
            Ok(runtime) => runtime,
            Err(err) => {
                tracing::warn!(
                    pane = pane_id.raw(),
                    terminal = %terminal_id,
                    agent = %plan.agent,
                    err = %err,
                    "failed to start shell for deferred agent resume"
                );
                // No process ran, so nothing replaced the saved conversation:
                // keep it for a later restart once the shell is fixed.
                return self.suspend_pending_agent_resume(
                    pane_id,
                    &terminal_id,
                    &format!("shell failed to start: {err}"),
                );
            }
        };

        if let Err(err) = queue_resumed_agent_launch(
            self.state.terminals.get_mut(&terminal_id),
            &runtime,
            &launch_env,
            &argv,
        ) {
            tracing::warn!(
                pane = pane_id.raw(),
                terminal = %terminal_id,
                agent = %plan.agent,
                err = %err,
                "failed to send deferred agent resume command to shell"
            );
            self.terminal_runtimes.insert(terminal_id.clone(), runtime);
            return self.suspend_pending_agent_resume(pane_id, &terminal_id, &err.to_string());
        }

        self.terminal_runtimes.insert(terminal_id.clone(), runtime);
        if let Some(terminal) = self.state.terminals.get_mut(&terminal_id) {
            terminal.pending_agent_resume_plan = None;
            terminal.respawn_shell_on_exit = false;
        }
        self.agent_resumes_in_flight
            .insert(terminal_id, AgentResumeInFlight::Launching);
        true
    }

    fn suspend_pending_agent_resume(
        &mut self,
        pane_id: crate::utils::ids::PaneId,
        terminal_id: &crate::utils::ids::TerminalId,
        reason: &str,
    ) -> bool {
        if reason == resume::RETIRED_OWNER {
            tracing::info!(event = "bus.resume.retired_owner", pane = pane_id.raw(), terminal = %terminal_id,
                "Bus conversation was kept after its agent was deleted");
        } else {
            tracing::warn!(event = "bus.resume.suspended", pane = pane_id.raw(), terminal = %terminal_id, %reason,
                "Bus conversation was kept; resume requires inspection before restarting");
        }
        if let Some(terminal) = self.state.terminals.get_mut(terminal_id) {
            // Keep durable name/session metadata for recovery on a later
            // restart, but do not retry disk reads on every layout tick.
            terminal.pending_agent_resume_plan = None;
            terminal.suspend_native_resume(reason);
            let kind = terminal.managed_agent_kind();
            terminal.set_detected_state_with_screen_signals_at(
                kind,
                crate::agents::AgentState::Unknown,
                false,
                false,
                None,
                Instant::now(),
            );
        }
        true
    }
}

fn queue_resumed_agent_launch(
    terminal: Option<&mut crate::terminal::TerminalState>,
    runtime: &crate::terminal::TerminalRuntime,
    launch_env: &crate::terminal::runtime::PaneLaunchEnv,
    argv: &[String],
) -> std::io::Result<()> {
    #[cfg(unix)]
    super::native_input::bind(runtime, launch_env);
    #[cfg(not(unix))]
    let _ = launch_env;
    if let Some(terminal) = terminal {
        guard_resumed_agent_launch(terminal, runtime);
    }
    #[cfg(unix)]
    let argv = &runtime.launch_shared_codex(argv.to_vec())?;
    runtime.queue_shell_command(argv, super::agents::DEFAULT_AGENT_START_TIMEOUT)
}

fn guard_resumed_agent_launch(
    terminal: &mut crate::terminal::TerminalState,
    runtime: &crate::terminal::TerminalRuntime,
) {
    let (Some(name), Some(kind)) = (terminal.agent_name.clone(), terminal.managed_agent_kind())
    else {
        return;
    };
    // The restored Idle label describes the old process. Require fresh
    // detection and the same readiness guard as AgentStart.
    let now = Instant::now();
    terminal.set_detected_state_with_screen_signals_at(
        None,
        crate::agents::AgentState::Unknown,
        false,
        false,
        None,
        now,
    );
    terminal.bind_managed_launch_pending(runtime.managed_launch_pending_signal());
    terminal.begin_managed_agent(
        name,
        kind,
        now,
        super::agents::AGENT_START_SETTLE_DELAY,
        super::agents::DEFAULT_AGENT_START_TIMEOUT,
    );
}

fn resume_launch_inputs(
    plan: &crate::agents::resume::catalog::AgentResumePlan,
    extras: LaunchExtras,
) -> (Vec<String>, Vec<(String, String)>) {
    use crate::agents::providers::launch;
    let mut argv = plan.argv.clone();
    let mut env = extras.env;
    if let Some(provider) = launch::native_provider(&plan.agent) {
        // Runtime safety is independent of room capture ownership. Keep the
        // executable outside argument insertion around a literal `--` tail.
        let mut args = argv.split_off(1);
        launch::extend_generated_args(provider, &mut args, extras.args);
        launch::extend_generated_args(provider, &mut args, launch::runtime_args(provider));
        argv.extend(args);
        let mut defaults = launch::runtime_env(provider);
        defaults.extend(env);
        env = defaults;
    } else {
        argv.extend(extras.args);
    }
    (argv, env)
}

fn derived_pending_agent_resume_pane_infos(
    tab: &crate::server::workspaces::Tab,
    terminal_area: Rect,
    pane_borders: crate::utils::config::PaneBordersConfig,
    pane_gaps: bool,
    pane_outer_borders: bool,
) -> Vec<crate::server::workspaces::layout::PaneInfo> {
    crate::server::rendering::surface::apply_pane_chrome(
        tab.layout.panes(terminal_area),
        pane_borders,
        pane_gaps,
        pane_outer_borders,
    )
    .into_iter()
    .map(|mut info| {
        let pane_inner =
            crate::server::rendering::surface::pane_inner_rect(info.rect, info.borders);
        info.inner_rect = stable_terminal_inner_rect(pane_inner);
        info
    })
    .collect()
}

fn stable_terminal_inner_rect(pane_inner: Rect) -> Rect {
    if pane_inner.width <= 4 {
        return pane_inner;
    }

    Rect::new(
        pane_inner.x,
        pane_inner.y,
        pane_inner.width.saturating_sub(1),
        pane_inner.height,
    )
}

fn shell_command_from_argv(argv: &[String]) -> String {
    argv.iter()
        .map(|part| shell_quote(part))
        .collect::<Vec<_>>()
        .join(" ")
}

fn resume_argv(
    plan: &crate::agents::resume::catalog::AgentResumePlan,
    extras: &LaunchExtras,
) -> Result<Vec<String>, String> {
    let mut argv = plan.argv.clone();
    if matches!(plan.agent.as_str(), "grok" | "kimi") && !extras.env.is_empty() {
        if argv.first().map(String::as_str) != Some(plan.agent.as_str()) {
            return Err("Provider resume plan has an unexpected executable".into());
        }
        let env = extras.env.iter().cloned().collect();
        argv[0] = crate::agents::providers::launch::selected_executable(
            if plan.agent == "grok" {
                crate::agents::providers::ProviderKind::Grok
            } else {
                crate::agents::providers::ProviderKind::Kimi
            },
            &env,
        )?
        .ok_or("Provider resume has no selected executable")?;
    }
    argv.extend(extras.args.iter().cloned());
    if plan.agent == "grok" && !extras.env.is_empty() {
        let home = extras
            .env
            .iter()
            .find(|(key, _)| key == "GROK_HOME")
            .map(|(_, value)| value)
            .ok_or("Grok resume has no validated home")?;
        argv = crate::agents::providers::grok::launch::bind_home(argv, std::path::Path::new(home))?;
    }
    if plan.agent == "kimi" && !extras.env.is_empty() {
        let home = extras
            .env
            .iter()
            .find(|(key, _)| key == "KIMI_CODE_HOME")
            .map(|(_, value)| value)
            .ok_or("Kimi resume has no private home")?;
        argv = crate::agents::providers::kimi::launch::bind_home(argv, std::path::Path::new(home))?;
    }
    Ok(argv)
}

#[cfg(test)]
fn resume_argv_for_test(
    plan: &crate::agents::resume::catalog::AgentResumePlan,
    env: Vec<(String, String)>,
    args: Vec<String>,
) -> Result<Vec<String>, String> {
    resume_argv(
        plan,
        &LaunchExtras {
            env,
            args,
            session: None,
        },
    )
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'_' | b'-' | b'.' | b'/' | b':' | b'@' | b'%' | b'+' | b'='
            )
    }) {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
fn terminal_resume_identity_for_test(
    terminal: &crate::terminal::TerminalState,
) -> (
    Option<String>,
    Option<String>,
    Option<crate::agents::resume::catalog::PersistedAgentSession>,
) {
    let facts = terminal_resume_facts(terminal);
    (facts.agent_name, facts.managed_agent, facts.session)
}

#[cfg(test)]
fn native_resume_launch_inputs_for_test(
    plan: &crate::agents::resume::catalog::AgentResumePlan,
    args: Vec<String>,
    env: Vec<(String, String)>,
) -> (Vec<String>, Vec<(String, String)>) {
    resume_launch_inputs(
        plan,
        LaunchExtras {
            args,
            env,
            ..Default::default()
        },
    )
}

#[cfg(test)]
fn prepare_native_resume_for_test(plan: &AgentResumePlan) -> Result<(), String> {
    PreparedResume::new(plan, LaunchExtras::default()).map(|_| ())
}

#[cfg(test)]
fn native_resume_is_empty_for_test(
    terminal: &crate::terminal::TerminalState,
    plan: &crate::agents::resume::catalog::AgentResumePlan,
    cwd: &std::path::Path,
) -> Result<bool, String> {
    let facts = terminal_resume_facts(terminal);
    match native_resume_context(&facts, cwd)? {
        Some(context) => {
            resume::load(&context, &facts, plan).map(|extras| extras == LaunchExtras::default())
        }
        None => Ok(true),
    }
}

#[cfg(all(test, unix))]
fn kept_resume_fixture_for_test(
    root: &std::path::Path,
    deleted: bool,
) -> (String, NativeResumeContext) {
    resume::kept_owner_fixture::save(root, deleted)
}

#[cfg(test)]
#[path = "tests/resume_test.rs"]
mod tests;

#[cfg(test)]
#[path = "resume/test_support_test.rs"]
mod test_support;

#[cfg(all(test, unix))]
#[path = "tests/codex_cold_launch_test.rs"]
mod codex_cold_launch;
