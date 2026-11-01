use std::time::{Duration, Instant};

use crate::protocol::api::schema::AgentStartParams;
use crate::server::{app::App, workspaces::ids::TerminalTargetError};

pub(super) const DEFAULT_AGENT_START_TIMEOUT: Duration = crate::protocol::api::AGENT_START_TIMEOUT;
pub(crate) const MAX_AGENT_START_TIMEOUT: Duration = Duration::from_secs(300);
pub(crate) const AGENT_START_SETTLE_DELAY: Duration = Duration::from_secs(3);
const INVALID_AGENT_TIMEOUT_MESSAGE: &str =
    "agent start timeout must be greater than 3000ms and at most 300000ms";
const INVALID_AGENT_NAME_MESSAGE: &str = "agent name must start with a lowercase letter and contain only lowercase letters, digits, '-' or '_' (1-32 characters)";

pub(crate) fn valid_agent_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some('a'..='z'))
        && name.len() <= 32
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '-' | '_'))
}

fn agent_start_argv(
    kind: crate::agents::AgentKind,
    selected_executable: Option<String>,
    grok_home: Option<String>,
    kimi_home: Option<String>,
    args: &[String],
) -> Result<Vec<String>, AgentStartError> {
    let mut args = args.to_vec();
    let launch =
        crate::agents::providers::launch::native_provider(crate::agents::agent_label(kind));
    if let Some(provider) = launch {
        crate::agents::providers::launch::extend_generated_args(
            provider,
            &mut args,
            crate::agents::providers::launch::runtime_args(provider),
        );
    }
    let executable = match (kind, selected_executable) {
        (crate::agents::AgentKind::Grok | crate::agents::AgentKind::Kimi, Some(path))
            if std::path::Path::new(&path).is_absolute()
                && std::path::Path::new(&path)
                    .file_name()
                    .is_some_and(|name| name == crate::agents::agent_label(kind))
                && !path.chars().any(char::is_control) =>
        {
            path
        }
        (crate::agents::AgentKind::Grok | crate::agents::AgentKind::Kimi, _) | (_, Some(_)) => {
            return Err(AgentStartError::InvalidArgument);
        }
        (_, None) => crate::agents::interactive_agent_executable(kind).to_string(),
    };
    let mut argv = vec![executable];
    argv.extend(args.iter().cloned());
    match (kind, grok_home) {
        (crate::agents::AgentKind::Grok, Some(home)) => {
            argv = crate::agents::providers::grok::launch::bind_home(
                argv,
                std::path::Path::new(&home),
            )
            .map_err(|_| AgentStartError::InvalidArgument)?;
        }
        (crate::agents::AgentKind::Grok, None) | (_, Some(_)) => {
            return Err(AgentStartError::InvalidArgument)
        }
        (_, None) => {}
    }
    match (kind, kimi_home) {
        (crate::agents::AgentKind::Kimi, Some(home)) => {
            argv = crate::agents::providers::kimi::launch::bind_home(
                argv,
                std::path::Path::new(&home),
            )
            .map_err(|_| AgentStartError::InvalidArgument)?;
        }
        (crate::agents::AgentKind::Kimi, None) | (_, Some(_)) => {
            return Err(AgentStartError::InvalidArgument)
        }
        (_, None) => {}
    }
    Ok(argv)
}

impl App {
    pub(in crate::server) fn collect_agent_infos(
        &self,
    ) -> Vec<crate::protocol::api::schema::AgentInfo> {
        self.state
            .workspaces
            .iter()
            .enumerate()
            .flat_map(|(ws_idx, ws)| {
                ws.tabs.iter().flat_map(move |tab| {
                    tab.layout
                        .pane_ids()
                        .into_iter()
                        .filter_map(move |pane_id| self.agent_info(ws_idx, pane_id))
                })
            })
            .collect()
    }

    pub(in crate::server) fn reconcile_managed_agent_target(&mut self, target: &str) {
        let Ok(resolved) = self.resolve_agent_target(target) else {
            return;
        };
        let Some(terminal_id) = self
            .state
            .workspaces
            .get(resolved.ws_idx)
            .and_then(|workspace| workspace.terminal_id(resolved.pane_id))
            .cloned()
        else {
            return;
        };
        let returned_to_prompt = self
            .terminal_runtimes
            .get(&terminal_id)
            .is_some_and(|runtime| {
                runtime.launch_returned_to_prompt()
                    && available_shell_name(runtime).is_some()
                    && live_runtime_agent(runtime).is_none()
            });
        let changed = self
            .state
            .terminals
            .get_mut(&terminal_id)
            .is_some_and(|terminal| {
                if terminal.managed_agent_awaiting_startup() && returned_to_prompt {
                    terminal.clear_agent_name();
                    true
                } else {
                    terminal.reconcile_managed_agent_at(Instant::now(), false)
                }
            });
        if changed {
            self.state.mark_session_dirty();
            self.schedule_session_save();
            self.emit_pane_updated(resolved.ws_idx, resolved.pane_id);
        }
    }

    pub(in crate::server) fn agent_info_for_target(
        &self,
        target: &str,
    ) -> Result<crate::protocol::api::schema::AgentInfo, TerminalTargetError> {
        let resolved = self.resolve_agent_target(target)?;
        self.agent_info(resolved.ws_idx, resolved.pane_id)
            .ok_or_else(|| TerminalTargetError::NotFound {
                target: target.to_string(),
            })
    }

    pub(in crate::server) fn focus_agent_target(
        &mut self,
        target: &str,
    ) -> Result<crate::protocol::api::schema::AgentInfo, TerminalTargetError> {
        let resolved = self.resolve_agent_target(target)?;
        self.state
            .focus_pane_in_workspace(resolved.ws_idx, resolved.pane_id);
        self.state.mark_active_tab_seen();
        self.state.mode = crate::server::app_settings::Mode::Terminal;
        self.agent_info(resolved.ws_idx, resolved.pane_id)
            .ok_or_else(|| TerminalTargetError::NotFound {
                target: target.to_string(),
            })
    }

    pub(in crate::server) fn rename_agent_target(
        &mut self,
        target: &str,
        name: Option<String>,
    ) -> Result<crate::protocol::api::schema::AgentInfo, AgentRenameError> {
        let resolved = self
            .resolve_agent_target(target)
            .map_err(AgentRenameError::Target)?;
        let normalized_name = match name {
            Some(name) if valid_agent_name(&name) => Some(name),
            Some(_) => return Err(AgentRenameError::InvalidName),
            None => None,
        };

        if let Some(name) = normalized_name.as_deref() {
            let conflicts = self.agent_name_conflicts(name, &resolved.terminal_id);
            if !conflicts.is_empty() {
                return Err(AgentRenameError::DuplicateName {
                    name: name.to_string(),
                    candidates: conflicts,
                });
            }
        }

        let Some(terminal) = self
            .state
            .terminals
            .values_mut()
            .find(|terminal| terminal.id.to_string() == resolved.terminal_id)
        else {
            return Err(AgentRenameError::Target(TerminalTargetError::NotFound {
                target: target.to_string(),
            }));
        };
        if terminal.managed_agent_awaiting_startup() {
            return Err(AgentRenameError::PendingLaunch);
        }
        if terminal.effective_agent_label().is_none() {
            return Err(AgentRenameError::NotAgent);
        }
        match normalized_name {
            Some(name) => terminal.set_agent_name(name),
            None => terminal.clear_agent_name(),
        }
        self.state.mark_session_dirty();
        self.schedule_session_save();
        self.emit_pane_updated(resolved.ws_idx, resolved.pane_id);
        self.agent_info(resolved.ws_idx, resolved.pane_id)
            .ok_or_else(|| {
                AgentRenameError::Target(TerminalTargetError::NotFound {
                    target: target.to_string(),
                })
            })
    }

    pub(in crate::server) fn start_agent(
        &mut self,
        params: AgentStartParams,
    ) -> Result<(crate::protocol::api::schema::AgentInfo, Vec<String>), AgentStartError> {
        let name = params.name;
        if !valid_agent_name(&name) {
            return Err(AgentStartError::InvalidName);
        }
        let Some(kind) = crate::agents::parse_agent_label(&params.kind) else {
            return Err(AgentStartError::UnsupportedKind(params.kind));
        };
        if params
            .args
            .iter()
            .any(|arg| arg.chars().any(char::is_control))
        {
            return Err(AgentStartError::InvalidArgument);
        }
        let persisted_agent_session =
            crate::agents::resume::catalog::persisted_session_from_launch_args(kind, &params.args);
        let conflicts = self.agent_name_conflicts(&name, "");
        if !conflicts.is_empty() {
            return Err(AgentStartError::DuplicateName {
                name,
                candidates: conflicts,
            });
        }
        let Some((ws_idx, pane_id)) = self.parse_current_public_pane_id(&params.pane_id) else {
            return Err(AgentStartError::TargetNotFound(params.pane_id));
        };
        let terminal_id = self
            .state
            .workspaces
            .get(ws_idx)
            .and_then(|workspace| workspace.terminal_id(pane_id))
            .cloned()
            .ok_or_else(|| AgentStartError::TargetNotFound(params.pane_id.clone()))?;
        let terminal = self
            .state
            .terminals
            .get(&terminal_id)
            .ok_or_else(|| AgentStartError::TargetNotFound(params.pane_id.clone()))?;
        if terminal.is_agent_terminal() || terminal.managed_agent_kind().is_some() {
            return Err(AgentStartError::TargetBusy(params.pane_id));
        }
        let runtime = self
            .terminal_runtimes
            .get(&terminal_id)
            .ok_or_else(|| AgentStartError::TargetUnavailableBeforeInput(params.pane_id.clone()))?;
        available_shell_name(runtime).ok_or_else(|| {
            if runtime.waiting_for_shell_prompt() {
                AgentStartError::ShellStarting(params.pane_id.clone())
            } else {
                AgentStartError::TargetBusy(params.pane_id.clone())
            }
        })?;

        let args = params.args;
        let kimi_code_home = params.kimi_home.as_ref().map(std::path::PathBuf::from);
        let argv = agent_start_argv(
            kind,
            params.executable,
            params.grok_home,
            params.kimi_home,
            &args,
        )?;
        let timeout = Duration::from_millis(
            params
                .timeout_ms
                .unwrap_or(DEFAULT_AGENT_START_TIMEOUT.as_millis() as u64),
        );
        if timeout <= AGENT_START_SETTLE_DELAY || timeout > MAX_AGENT_START_TIMEOUT {
            return Err(AgentStartError::InvalidTimeout);
        }

        let now = Instant::now();
        let terminal =
            self.state.terminals.get_mut(&terminal_id).ok_or_else(|| {
                AgentStartError::TargetUnavailableBeforeInput(params.pane_id.clone())
            })?;
        terminal.bind_managed_launch_pending(runtime.managed_launch_pending_signal());
        terminal.begin_managed_agent(name.clone(), kind, now, AGENT_START_SETTLE_DELAY, timeout);
        runtime.mark_agent_launch();
        if let Err(err) = runtime.queue_shell_command(&argv, timeout) {
            terminal.clear_agent_name();
            return Err(if err.kind() == std::io::ErrorKind::Unsupported {
                AgentStartError::NoShellEditor(err.to_string())
            } else {
                AgentStartError::InputFailed(err.to_string())
            });
        }
        runtime.bind_kimi_code_home(kimi_code_home);
        if let Some(session) = persisted_agent_session {
            terminal.set_managed_agent_launch_session(session);
        }
        self.state.mark_session_dirty();
        self.schedule_session_save();

        let agent = self
            .agent_info(ws_idx, pane_id)
            .ok_or(AgentStartError::TargetUnavailable(params.pane_id))?;
        Ok((agent, argv))
    }

    pub(in crate::server) fn agent_start_error_body(
        &self,
        err: AgentStartError,
    ) -> crate::protocol::api::schema::ErrorBody {
        match err {
            AgentStartError::InvalidName => crate::protocol::api::schema::ErrorBody {
                code: "invalid_agent_name".into(),
                message: INVALID_AGENT_NAME_MESSAGE.into(),
            },
            AgentStartError::UnsupportedKind(kind) => crate::protocol::api::schema::ErrorBody {
                code: "unsupported_agent_kind".into(),
                message: format!("unsupported interactive agent kind {kind}"),
            },
            AgentStartError::InvalidArgument => crate::protocol::api::schema::ErrorBody {
                code: "invalid_agent_argument".into(),
                message: "agent arguments cannot be encoded safely for the target shell".into(),
            },
            AgentStartError::InvalidTimeout => crate::protocol::api::schema::ErrorBody {
                code: "invalid_agent_timeout".into(),
                message: INVALID_AGENT_TIMEOUT_MESSAGE.into(),
            },
            AgentStartError::TargetNotFound(target) => crate::protocol::api::schema::ErrorBody {
                code: "agent_pane_not_found".into(),
                message: format!("agent target pane {target} not found"),
            },
            AgentStartError::TargetBusy(target) => crate::protocol::api::schema::ErrorBody {
                code: "agent_pane_busy".into(),
                message: format!("agent target pane {target} is not an available shell. Wait for its prompt; answer any shell startup question in the terminal, then retry the launch."),
            },
            AgentStartError::ShellStarting(target) => self.shell_wait_error_body(&target),
            AgentStartError::TargetUnavailableBeforeInput(target) => crate::protocol::api::schema::ErrorBody {
                code: "agent_pane_unavailable_before_input".into(),
                message: format!("agent target pane {target} has no live terminal; no launch input was sent"),
            },
            AgentStartError::TargetUnavailable(target) => crate::protocol::api::schema::ErrorBody {
                code: "agent_pane_unavailable".into(),
                message: format!("agent target pane {target} has no live terminal"),
            },
            AgentStartError::NoShellEditor(message) => crate::protocol::api::schema::ErrorBody {
                code: "agent_shell_integration_unavailable".into(), message,
            },
            AgentStartError::InputFailed(message) => crate::protocol::api::schema::ErrorBody {
                code: "agent_start_input_failed".into(),
                message,
            },
            AgentStartError::DuplicateName { name, candidates } => crate::protocol::api::schema::ErrorBody {
                code: "agent_name_taken".into(),
                message: format!(
                    "agent name {name} is already used; candidates: {}",
                    candidates
                        .into_iter()
                        .map(|candidate| format!(
                            "terminal_id={} pane_id={} workspace_id={} tab_id={} cwd={} status={:?}",
                            candidate.terminal_id,
                            candidate.pane_id,
                            candidate.workspace_id,
                            candidate.tab_id,
                            candidate.cwd.unwrap_or_else(|| "unknown".into()),
                            candidate.agent_status,
                        ))
                        .collect::<Vec<_>>()
                        .join("; ")
                ),
            },
        }
    }

    fn shell_wait_error_body(&self, target: &str) -> crate::protocol::api::schema::ErrorBody {
        let runtime = self
            .parse_current_public_pane_id(target)
            .and_then(|(workspace, pane)| self.state.workspaces.get(workspace)?.terminal_id(pane))
            .and_then(|id| self.terminal_runtimes.get(id));
        let expired = runtime.is_some_and(|runtime| runtime.shell_prompt_wait_expired());
        let last_line = runtime
            .map(|runtime| {
                runtime
                    .visible_text()
                    .lines()
                    .rev()
                    .find(|line| !line.trim().is_empty())
                    .map(|line| {
                        line.chars()
                            .filter(|c| !c.is_control() || *c == '\t')
                            .take(300)
                            .collect::<String>()
                    })
                    .unwrap_or_else(|| "(no visible output)".into())
            })
            .unwrap_or_else(|| "(pane unavailable)".into());
        crate::protocol::api::schema::ErrorBody {
            code: if expired { "agent_shell_waiting" } else { "agent_shell_starting" }.into(),
            message: format!(
                "Shell waiting for input or prompt confirmation{}. Last visible line: {last_line}. Answer any shell startup question, then resume this agent.",
                if expired { " after 30 seconds" } else { "" },
            ),
        }
    }

    pub(in crate::server) fn agent_target_error_body(
        &self,
        err: TerminalTargetError,
    ) -> crate::protocol::api::schema::ErrorBody {
        match err {
            TerminalTargetError::NotFound { target } => crate::protocol::api::schema::ErrorBody {
                code: "agent_not_found".into(),
                message: format!("agent target {target} not found"),
            },
            TerminalTargetError::Ambiguous { target, candidates } => {
                crate::protocol::api::schema::ErrorBody {
                    code: "agent_target_ambiguous".into(),
                    message: format!(
                        "agent target {target} is ambiguous; candidates: {}",
                        candidates
                            .into_iter()
                            .map(|candidate| format!(
                                "terminal_id={} pane_id={} workspace_id={} tab_id={} cwd={} status={:?}",
                                candidate.terminal_id,
                                candidate.pane_id,
                                candidate.workspace_id,
                                candidate.tab_id,
                                candidate.cwd.unwrap_or_else(|| "unknown".into()),
                                candidate.agent_status,
                            ))
                            .collect::<Vec<_>>()
                            .join("; ")
                    ),
                }
            }
        }
    }

    pub(in crate::server) fn agent_rename_error_body(
        &self,
        err: AgentRenameError,
    ) -> crate::protocol::api::schema::ErrorBody {
        match err {
            AgentRenameError::Target(err) => self.agent_target_error_body(err),
            AgentRenameError::InvalidName => crate::protocol::api::schema::ErrorBody {
                code: "invalid_agent_name".into(),
                message: INVALID_AGENT_NAME_MESSAGE.into(),
            },
            AgentRenameError::NotAgent => crate::protocol::api::schema::ErrorBody {
                code: "agent_not_found".into(),
                message: "agent target does not currently host an agent".into(),
            },
            AgentRenameError::PendingLaunch => crate::protocol::api::schema::ErrorBody {
                code: "agent_launch_pending".into(),
                message: "agent name cannot change while startup is pending".into(),
            },
            AgentRenameError::DuplicateName { name, candidates } => crate::protocol::api::schema::ErrorBody {
                code: "agent_name_taken".into(),
                message: format!(
                    "agent name {name} is already used; candidates: {}",
                    candidates
                        .into_iter()
                        .map(|candidate| format!(
                            "terminal_id={} pane_id={} workspace_id={} tab_id={} cwd={} status={:?}",
                            candidate.terminal_id,
                            candidate.pane_id,
                            candidate.workspace_id,
                            candidate.tab_id,
                            candidate.cwd.unwrap_or_else(|| "unknown".into()),
                            candidate.agent_status,
                        ))
                        .collect::<Vec<_>>()
                        .join("; ")
                ),
            },
        }
    }

    pub(in crate::server) fn agent_info(
        &self,
        ws_idx: usize,
        pane_id: crate::utils::ids::PaneId,
    ) -> Option<crate::protocol::api::schema::AgentInfo> {
        let ws = self.state.workspaces.get(ws_idx)?;
        let pane_state = ws.pane_state(pane_id)?;
        let terminal = self.state.terminals.get(&pane_state.attached_terminal_id)?;
        if !terminal.is_agent_terminal() && terminal.native_resume.is_none() {
            return None;
        }
        let pane = self.pane_info(ws_idx, pane_id)?;
        let dialog = self
            .lookup_runtime_sender(ws_idx, pane_id)
            .and_then(|runtime| runtime.visible_ansi_snapshot_with_seq())
            .and_then(|(screen, _)| {
                crate::agents::dialog::parse(&screen)
                    .map(|dialog| (dialog.id(), true))
                    .or_else(|| {
                        (terminal.effective_known_agent() == Some(crate::agents::AgentKind::Codex)
                            && crate::agents::dialog::codex_question_pending(&screen))
                        .then(|| {
                            (
                                crate::agents::dialog::CODEX_PENDING_QUESTION_ID.to_owned(),
                                false,
                            )
                        })
                    })
            });
        Some(crate::protocol::api::schema::AgentInfo {
            launch_input_error: self
                .lookup_runtime_sender(ws_idx, pane_id)
                .and_then(|runtime| runtime.definite_launch_input_error()),
            native_resume: terminal.native_resume.as_ref().map(|resume| {
                crate::protocol::api::schema::NativeResumeInfo {
                    name: resume.name.clone(),
                    agent: crate::agents::agent_label(resume.kind).into(),
                    session_id: resume
                        .session
                        .as_ref()
                        .map(|session| session.session_ref.value.clone()),
                    command: resume.command.clone(),
                    error: resume.error.clone(),
                }
            }),
            terminal_id: pane.terminal_id,
            name: terminal.agent_name.clone(),
            agent: pane.agent,
            title: pane.title,
            terminal_title: pane.terminal_title,
            terminal_title_stripped: pane.terminal_title_stripped,
            display_agent: pane.display_agent,
            agent_status: pane.agent_status,
            screen_detection_skipped: false,
            dialog_id: dialog.map(|(id, _)| id),
            update_error: None,
            state_labels: pane.state_labels,
            tokens: pane.tokens,
            agent_session: pane.agent_session,
            workspace_id: pane.workspace_id,
            tab_id: pane.tab_id,
            pane_id: pane.pane_id,
            focused: pane.focused,
            launch_pending: terminal.pending_agent_resume_plan.is_some()
                || terminal.managed_agent_launch_pending(),
            interactive_ready: terminal.pending_agent_resume_plan.is_none()
                && terminal.managed_agent_interactive_ready(),
            native_ready: terminal.pending_agent_resume_plan.is_none()
                && !terminal.managed_agent_launch_pending()
                && terminal.effective_known_agent().is_some_and(|expected| {
                    self.lookup_runtime_sender(ws_idx, pane_id)
                        .is_some_and(|runtime| runtime_hosts_agent(runtime, expected))
                }),
            state_change_seq: terminal.last_agent_state_change_seq.unwrap_or(0),
            cwd: pane.cwd,
            foreground_cwd: pane.foreground_cwd,
            revision: pane.revision,
        })
    }

    fn agent_name_conflicts(
        &self,
        name: &str,
        except_terminal_id: &str,
    ) -> Vec<crate::protocol::api::schema::AgentInfo> {
        self.collect_agent_infos()
            .into_iter()
            .filter(|agent| {
                agent.name.as_deref() == Some(name) && agent.terminal_id != except_terminal_id
            })
            .collect()
    }
}

pub(in crate::server) fn available_shell_name(
    runtime: &crate::terminal::TerminalRuntime,
) -> Option<String> {
    if runtime.waiting_for_shell_prompt() {
        return None;
    }
    #[cfg(test)]
    if runtime.child_pid().is_none() {
        return Some("sh".into());
    }
    crate::platform::available_pane_shell(runtime.child_pid()?)
}

/// Whether the terminal session holds nothing but its idle shell: the shell
/// is in the foreground and no other process, foreground or background job,
/// shares its session. An unreadable session counts as busy.
pub(in crate::server) fn runtime_is_idle_shell(runtime: &crate::terminal::TerminalRuntime) -> bool {
    #[cfg(test)]
    if runtime.child_pid().is_none() {
        return true;
    }
    let Some(shell) = runtime.child_pid() else {
        return false;
    };
    available_shell_name(runtime).is_some() && crate::platform::session_processes(shell) == [shell]
}

pub(in crate::server) fn runtime_hosts_agent(
    runtime: &crate::terminal::TerminalRuntime,
    expected: crate::agents::AgentKind,
) -> bool {
    #[cfg(test)]
    if runtime.child_pid().is_none() {
        return true;
    }
    live_runtime_agent(runtime) == Some(expected)
}

fn live_runtime_agent(
    runtime: &crate::terminal::TerminalRuntime,
) -> Option<crate::agents::AgentKind> {
    let job = crate::agents::foreground_job(runtime.child_pid()?)?;
    crate::agents::identify_agent_in_job(&job)
        .map(|(agent, _)| agent)
        .or_else(|| {
            job.processes
                .iter()
                .find_map(|process| crate::agents::process_agent_hint(process.pid))
        })
}

pub(in crate::server) enum AgentStartError {
    ShellStarting(String),
    InvalidName,
    UnsupportedKind(String),
    InvalidArgument,
    InvalidTimeout,
    TargetNotFound(String),
    TargetBusy(String),
    TargetUnavailable(String),
    TargetUnavailableBeforeInput(String),
    InputFailed(String),
    NoShellEditor(String),
    DuplicateName {
        name: String,
        candidates: Vec<crate::protocol::api::schema::AgentInfo>,
    },
}

pub(in crate::server) enum AgentRenameError {
    Target(TerminalTargetError),
    InvalidName,
    NotAgent,
    PendingLaunch,
    DuplicateName {
        name: String,
        candidates: Vec<crate::protocol::api::schema::AgentInfo>,
    },
}

#[cfg(test)]
mod tests {
    use super::valid_agent_name;

    #[tokio::test]
    async fn native_provider_starts_disable_updates_without_capture_ownership() {
        for (label, flag) in [
            ("codex", "check_for_update_on_startup=false"),
            ("cursor", "--disable-auto-update"),
        ] {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            let _tx = tx;
            let mut app = crate::server::app::App::new(
                &crate::utils::config::Config::default(),
                crate::server::app::AppPolicy::TEST,
                None,
                rx,
                crate::server::api::EventHub::default(),
            );
            let workspace = crate::server::workspaces::Workspace::test_new("native-start");
            let pane = workspace.tabs[0].root_pane;
            let terminal = workspace.terminal_id(pane).cloned().unwrap();
            app.state.workspaces = vec![workspace];
            app.state.ensure_test_terminals();
            app.state.active = Some(0);
            let (runtime, mut writes) =
                crate::terminal::TerminalRuntime::test_with_channel_capacity(100, 30, 8);
            app.terminal_runtimes.insert(terminal, runtime);
            let pane_id = app.public_pane_id(0, pane).unwrap();
            let result = app.start_agent(crate::protocol::api::schema::AgentStartParams {
                name: "native-worker".into(),
                kind: label.into(),
                pane_id,
                args: vec![],
                executable: None,
                grok_home: None,
                kimi_home: None,
                timeout_ms: None,
            });
            assert!(result.is_ok());
            let bytes = writes.try_recv().unwrap();
            let command = String::from_utf8_lossy(&bytes);
            assert!(
                command.contains(flag),
                "{label} started without update suppression: {command}"
            );
        }
    }

    #[test]
    fn agent_names_use_a_small_cli_safe_grammar() {
        for name in ["a", "reviewer-one", "reviewer_2", &"a".repeat(32)] {
            assert!(valid_agent_name(name), "expected {name:?} to be valid");
        }
        for name in [
            "",
            " reviewer",
            "reviewer ",
            "reviewer one",
            "Reviewer",
            "1reviewer",
            "reviewer.one",
            &"a".repeat(33),
        ] {
            assert!(!valid_agent_name(name), "expected {name:?} to be invalid");
        }
    }
}
