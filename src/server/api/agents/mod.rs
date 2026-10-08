use std::time::Duration;

use bytes::Bytes;

use crate::protocol::api::schema::{
    AgentDialog, AgentDialogAnswerParams, AgentDialogChooseParams, AgentDialogChooseResult,
    AgentDialogKind, AgentDialogObservation, AgentDialogOption, AgentPromptParams,
    AgentRenameParams, AgentSendKeysParams, AgentStartParams, AgentTarget, PaneReadResult,
    ResponseResult,
};
use crate::server::app::App;

use super::errors::{encode_error, encode_error_body, encode_success};

const AGENT_PROMPT_SUBMIT_DELAY: Duration = Duration::from_millis(300);

fn codex_composer_is_empty(screen: &str) -> bool {
    let Some(body) = screen
        .lines()
        .rev()
        .find_map(|line| line.trim_start().strip_prefix('›').map(|body| body.trim()))
    else {
        return false;
    };

    body.is_empty()
        || matches!(
            body,
            "Ask Codex to do anything" | "Use /skills to list available skills"
        )
}

/// Whether Claude Code's input box (the `❯` line between the two rules at the
/// bottom of its screen, plus any wrapped lines down to the closing rule)
/// holds no typed text. Typing into a box the Human already wrote in would
/// merge both into one prompt Bus cannot match, or be swallowed, so a Bus
/// delivery waits instead. Claude's dimmed prompt suggestion and the inverse
/// cursor cell are not typed text. No visible box counts as not empty.
fn claude_input_is_empty(screen_ansi: &str) -> bool {
    let lines: Vec<Vec<(char, bool)>> = screen_ansi.lines().map(styled_chars).collect();
    let plain = |line: &[(char, bool)]| line.iter().map(|(c, _)| *c).collect::<String>();
    let is_rule = |line: &[(char, bool)]| plain(line).trim().starts_with('─');
    let Some(start) = (1..lines.len()).rev().find(|&index| {
        is_rule(&lines[index - 1]) && plain(&lines[index]).trim_start().starts_with('❯')
    }) else {
        return false;
    };
    let mut body = lines[start..].iter().take_while(|line| !is_rule(line));
    let first = body
        .next()
        .into_iter()
        .flat_map(|line| line.iter().skip_while(|(c, _)| *c != '❯').skip(1));
    first
        .chain(body.flatten())
        .all(|(c, faint)| *faint || c.is_whitespace())
}

/// Each visible character of an ANSI screen line, with whether it is drawn
/// dimmed or inverse.
fn styled_chars(line: &str) -> Vec<(char, bool)> {
    let mut out = Vec::new();
    let (mut dim, mut inverse) = (false, false);
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push((c, dim || inverse));
            continue;
        }
        match chars.next() {
            Some('[') => {
                let mut params = String::new();
                let mut last = None;
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        last = Some(c);
                        break;
                    }
                    params.push(c);
                }
                if last == Some('m') {
                    apply_sgr(&params, &mut dim, &mut inverse);
                }
            }
            // OSC (hyperlinks): skip to BEL or ST.
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn apply_sgr(params: &str, dim: &mut bool, inverse: &mut bool) {
    let params: Vec<&str> = params.split(';').collect();
    let mut index = 0;
    while index < params.len() {
        match params[index] {
            "" | "0" => (*dim, *inverse) = (false, false),
            "2" => *dim = true,
            "22" => *dim = false,
            "7" => *inverse = true,
            "27" => *inverse = false,
            // Extended colors carry their own numbers (38;2;R;G;B or 38;5;N),
            // which must not read as dim or inverse.
            "38" | "48" | "58" => {
                index += match params.get(index + 1) {
                    Some(&"5") => 2,
                    Some(&"2") => 4,
                    _ => 0,
                }
            }
            _ => {}
        }
        index += 1;
    }
}

fn check_unbound_prompt_identity_and_idle(
    agent: &crate::protocol::api::schema::AgentInfo,
    params: &crate::protocol::api::schema::AgentPromptIfUnboundParams,
) -> Result<(), (&'static str, &'static str)> {
    if agent.terminal_id != params.expected_terminal_id
        || agent.pane_id != params.expected_pane_id
        || agent.agent.as_deref() != Some("codex")
        || agent.name.as_deref() != Some(params.expected_managed_name.as_str())
        || params.expected_managed_name.is_empty()
        || agent.agent_session.is_some()
    {
        return Err((
            "agent_identity_changed",
            "Managed launch identity changed; prompt was not sent",
        ));
    }
    if !matches!(
        agent.agent_status,
        crate::protocol::api::schema::AgentStatus::Idle
            | crate::protocol::api::schema::AgentStatus::Done
    ) {
        return Err(("agent_not_idle", "Agent is not idle; prompt was not sent"));
    }
    if agent.launch_pending || !agent.interactive_ready {
        return Err(("agent_not_ready", "Agent is not ready; prompt was not sent"));
    }
    Ok(())
}

fn check_prompt_identity_and_idle(
    agent: &crate::protocol::api::schema::AgentInfo,
    params: &crate::protocol::api::schema::AgentPromptIfIdleParams,
) -> Result<(), (&'static str, &'static str)> {
    use crate::protocol::api::schema::AgentStatus;
    if agent.terminal_id != params.expected_terminal_id
        || agent.pane_id != params.expected_pane_id
        || agent.agent.as_deref() != Some(params.expected_agent.as_str())
        || agent
            .agent_session
            .as_ref()
            .map(|session| session.value.as_str())
            != Some(params.expected_session_id.as_str())
        || params.expected_session_id.is_empty()
    {
        return Err((
            "agent_identity_changed",
            "Agent identity changed; prompt was not sent",
        ));
    }
    let accepted = match agent.agent_status {
        AgentStatus::Idle | AgentStatus::Done => true,
        AgentStatus::Working => params.steer,
        AgentStatus::Blocked | AgentStatus::Unknown => false,
    };
    if !accepted || agent.dialog_id.is_some() {
        return Err(("agent_not_idle", "Agent is not idle; prompt was not sent"));
    }
    if agent.launch_pending || !agent.interactive_ready {
        return Err(("agent_not_ready", "Agent is not ready; prompt was not sent"));
    }
    Ok(())
}

fn agent_prompt_submit_delay(agent: crate::detect::Agent, prompt_bytes: usize) -> Duration {
    #[cfg(windows)]
    if agent == crate::detect::Agent::Codex {
        // Codex consumes Windows paste bursts at about 4 bytes/ms, then suppresses Enter briefly.
        // ponytail: best-effort ConPTY timing; remove when Codex exposes a paste-complete boundary.
        return Duration::from_millis(600 + prompt_bytes as u64 / 4);
    }
    #[cfg(not(windows))]
    let _ = (agent, prompt_bytes);
    AGENT_PROMPT_SUBMIT_DELAY
}

impl App {
    pub(super) fn handle_agent_list(&mut self, id: String) -> String {
        encode_success(
            id,
            ResponseResult::AgentList {
                agents: self.collect_agent_infos(),
            },
        )
    }

    pub(super) fn handle_agent_get(&mut self, id: String, target: AgentTarget) -> String {
        self.reconcile_managed_agent_target(&target.target);
        let agent = match self.agent_info_for_target(&target.target) {
            Ok(agent) => agent,
            Err(err) => return encode_error_body(id, self.agent_target_error_body(err)),
        };

        encode_success(id, ResponseResult::AgentInfo { agent })
    }

    pub(super) fn handle_agent_focus(&mut self, id: String, target: AgentTarget) -> String {
        let agent = match self.focus_agent_target(&target.target) {
            Ok(agent) => agent,
            Err(err) => return encode_error_body(id, self.agent_target_error_body(err)),
        };

        encode_success(id, ResponseResult::AgentInfo { agent })
    }

    pub(super) fn handle_agent_rename(&mut self, id: String, params: AgentRenameParams) -> String {
        let agent = match self.rename_agent_target(&params.target, params.name) {
            Ok(agent) => agent,
            Err(err) => return encode_error_body(id, self.agent_rename_error_body(err)),
        };

        encode_success(id, ResponseResult::AgentInfo { agent })
    }

    pub(super) fn handle_agent_start(&mut self, id: String, params: AgentStartParams) -> String {
        let (agent, argv) = match self.start_agent(params) {
            Ok(started) => started,
            Err(err) => return encode_error_body(id, self.agent_start_error_body(err)),
        };

        encode_success(id, ResponseResult::AgentStarted { agent, argv })
    }

    pub(crate) fn handle_deferred_agent_api_request(
        &mut self,
        request: crate::protocol::api::schema::Request,
        respond_to: std::sync::mpsc::Sender<String>,
    ) -> bool {
        let api_request_id = request.id.clone();
        let started = std::time::Instant::now();
        let queued = match request.method {
            crate::protocol::api::schema::Method::AgentPrompt(params) => {
                self.queue_agent_prompt(request.id, params)
            }
            crate::protocol::api::schema::Method::AgentPromptIfIdle(params) => {
                self.queue_agent_prompt_if_idle(request.id, params)
            }
            crate::protocol::api::schema::Method::AgentPromptIfUnbound(params) => {
                self.queue_agent_prompt_if_unbound(request.id, params)
            }
            _ => return false,
        };
        match queued {
            Ok((id, agent, completion)) => {
                tracing::debug!(event = "bus.terminal.queued", api_request_id = %id,
                    pane_id = %agent.pane_id, terminal_id = %agent.terminal_id,
                    "Prompt submission queued in native terminal writer");
                std::thread::spawn(move || {
                    let completed = completion.recv();
                    let outcome = match &completed {
                        Ok(Ok(())) => "written",
                        Ok(Err(err)) if err.kind() == std::io::ErrorKind::TimedOut => "timeout",
                        Ok(Err(_)) => "write_failed",
                        Err(_) => "writer_closed",
                    };
                    tracing::info!(event = "bus.terminal.result", api_request_id = %id,
                        pane_id = %agent.pane_id, terminal_id = %agent.terminal_id, outcome,
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        "Native prompt and Enter submission completed; this is not model acceptance");
                    let response = match completed {
                        Ok(Ok(())) => encode_success(id, ResponseResult::AgentPrompted { agent }),
                        Ok(Err(err)) if err.kind() == std::io::ErrorKind::TimedOut => {
                            encode_error(id, "timeout", err.to_string())
                        }
                        Ok(Err(err)) => encode_error(id, "agent_prompt_failed", err.to_string()),
                        Err(_) => encode_error(id, "agent_prompt_failed", "pty actor closed"),
                    };
                    let _ = respond_to.send(response);
                });
            }
            Err(response) => {
                tracing::info!(event = "bus.terminal.rejected", api_request_id = %api_request_id,
                    "Native submit rejected; API response contains the error code");
                let _ = respond_to.send(response);
            }
        }
        true
    }

    fn queue_agent_prompt_if_unbound(
        &mut self,
        id: String,
        params: crate::protocol::api::schema::AgentPromptIfUnboundParams,
    ) -> Result<
        (
            String,
            crate::protocol::api::schema::AgentInfo,
            std::sync::mpsc::Receiver<std::io::Result<()>>,
        ),
        String,
    > {
        self.reconcile_managed_agent_target(&params.target);
        let agent = self
            .agent_info_for_target(&params.target)
            .map_err(|err| encode_error_body(id.clone(), self.agent_target_error_body(err)))?;
        if let Err((code, message)) = check_unbound_prompt_identity_and_idle(&agent, &params) {
            return Err(encode_error(id, code, message));
        }
        self.queue_agent_prompt(
            id,
            AgentPromptParams {
                target: params.target,
                text: params.text,
                wait: None,
            },
        )
    }

    fn queue_agent_prompt_if_idle(
        &mut self,
        id: String,
        params: crate::protocol::api::schema::AgentPromptIfIdleParams,
    ) -> Result<
        (
            String,
            crate::protocol::api::schema::AgentInfo,
            std::sync::mpsc::Receiver<std::io::Result<()>>,
        ),
        String,
    > {
        self.reconcile_managed_agent_target(&params.target);
        let agent = self
            .agent_info_for_target(&params.target)
            .map_err(|err| encode_error_body(id.clone(), self.agent_target_error_body(err)))?;
        if let Err((code, message)) = check_prompt_identity_and_idle(&agent, &params) {
            return Err(encode_error(id, code, message));
        }
        if agent.agent.as_deref() == Some("codex") {
            let resolved = self
                .resolve_agent_target(&params.target)
                .map_err(|err| encode_error_body(id.clone(), self.agent_target_error_body(err)))?;
            let Some(runtime) = self.lookup_runtime_sender(resolved.ws_idx, resolved.pane_id)
            else {
                return Err(agent_not_found(id, &params.target));
            };
            if !codex_composer_is_empty(&runtime.visible_text()) {
                return Err(encode_error(
                    id,
                    "agent_not_ready",
                    "Codex composer is not empty; prompt was not sent",
                ));
            }
        }
        if agent.agent.as_deref() == Some("claude") {
            let resolved = self
                .resolve_agent_target(&params.target)
                .map_err(|err| encode_error_body(id.clone(), self.agent_target_error_body(err)))?;
            let Some(runtime) = self.lookup_runtime_sender(resolved.ws_idx, resolved.pane_id)
            else {
                return Err(agent_not_found(id, &params.target));
            };
            // A rejection keeps the Bus message queued for a later attempt.
            if !claude_input_is_empty(&runtime.visible_ansi()) {
                return Err(encode_error(
                    id,
                    "agent_not_ready",
                    "Claude input box is not empty; prompt was not sent",
                ));
            }
        }
        self.queue_agent_prompt(
            id,
            AgentPromptParams {
                target: params.target,
                text: params.text,
                wait: None,
            },
        )
    }

    fn queue_agent_prompt(
        &mut self,
        id: String,
        params: AgentPromptParams,
    ) -> Result<
        (
            String,
            crate::protocol::api::schema::AgentInfo,
            std::sync::mpsc::Receiver<std::io::Result<()>>,
        ),
        String,
    > {
        if params.text.is_empty() {
            return Err(encode_error(
                id,
                "empty_agent_prompt",
                "agent prompt must not be empty",
            ));
        }
        let resolved = match self.resolve_agent_target(&params.target) {
            Ok(resolved) => resolved,
            Err(err) => return Err(encode_error_body(id, self.agent_target_error_body(err))),
        };
        let Some(terminal_id) = self
            .state
            .workspaces
            .get(resolved.ws_idx)
            .and_then(|workspace| workspace.terminal_id(resolved.pane_id))
            .cloned()
        else {
            return Err(agent_not_found(id, &params.target));
        };
        let Some(terminal) = self.state.terminals.get(&terminal_id) else {
            return Err(agent_not_found(id, &params.target));
        };
        if terminal.state == crate::detect::AgentState::Blocked {
            return Err(encode_error(
                id,
                "agent_blocked",
                format!(
                    "agent {} is blocked and requires interactive input",
                    params.target
                ),
            ));
        }
        let Some(expected_agent) = terminal.effective_known_agent() else {
            return Err(agent_not_ready(id, &params.target));
        };
        if terminal.managed_agent_launch_pending() {
            return Err(agent_not_ready(id, &params.target));
        }
        let Some(runtime) = self.lookup_runtime_sender(resolved.ws_idx, resolved.pane_id) else {
            return Err(agent_not_found(id, &params.target));
        };
        if !crate::server::terminals::agents::runtime_hosts_agent(runtime, expected_agent) {
            return Err(encode_error(
                id,
                "agent_not_ready",
                format!(
                    "agent {} is no longer the pane foreground process",
                    params.target
                ),
            ));
        }
        let submit_delay = agent_prompt_submit_delay(expected_agent, params.text.len());
        #[cfg(windows)]
        let submit_deadline = params
            .wait
            .as_ref()
            .and_then(|wait| wait.submission_deadline);
        #[cfg(not(windows))]
        let submit_deadline = None;
        if expected_agent == crate::detect::Agent::GithubCopilot {
            // Copilot ignores synthetic Enter after focus loss until it receives focus gained.
            let focus = match crate::ghostty::encode_focus(crate::ghostty::FocusEvent::Gained) {
                Ok(focus) => focus,
                Err(err) => {
                    return Err(encode_error(id, "agent_prompt_failed", err.to_string()));
                }
            };
            if let Err(err) = runtime.try_send_bytes(Bytes::from(focus)) {
                return Err(encode_error(id, "agent_prompt_failed", err.to_string()));
            }
        }
        let (text, enter) =
            crate::server::api::input_encoding::encode_api_submission_parts(runtime, &params.text);
        let Some(agent) = self.agent_info(resolved.ws_idx, resolved.pane_id) else {
            return Err(agent_not_found(id, &params.target));
        };
        let completion = runtime
            .queue_user_input_submission(
                Bytes::from(text),
                Bytes::from(enter),
                submit_delay,
                submit_deadline,
            )
            .map_err(|err| encode_error(id.clone(), "agent_prompt_failed", err.to_string()))?;
        Ok((id, agent, completion))
    }

    pub(super) fn handle_agent_read(
        &mut self,
        id: String,
        params: crate::protocol::api::schema::AgentReadParams,
    ) -> String {
        let resolved = match self.resolve_agent_target(&params.target) {
            Ok(resolved) => resolved,
            Err(err) => return encode_error_body(id, self.agent_target_error_body(err)),
        };
        let Some((pane, workspace_id)) = self.lookup_runtime(resolved.ws_idx, resolved.pane_id)
        else {
            return agent_not_found(id, &params.target);
        };
        let snapshot = crate::server::api::input_encoding::read_terminal_snapshot(
            pane,
            params.source,
            params.format,
            params.lines,
        );

        encode_success(
            id,
            ResponseResult::PaneRead {
                read: PaneReadResult {
                    pane_id: self
                        .public_pane_id(resolved.ws_idx, resolved.pane_id)
                        .unwrap_or_else(|| params.target.clone()),
                    workspace_id,
                    tab_id: self
                        .public_tab_id(resolved.ws_idx, resolved.tab_idx)
                        .unwrap(),
                    source: params.source,
                    format: params.format,
                    text: snapshot.text,
                    revision: snapshot.revision,
                    truncated: snapshot.truncated,
                    viewport_rows: snapshot.viewport_rows,
                    viewport_columns: snapshot.viewport_columns,
                    requested_lines: snapshot.requested_lines,
                    returned_lines: snapshot.returned_lines,
                    available_lines: snapshot.available_lines,
                    exhausted: snapshot.exhausted,
                },
            },
        )
    }

    pub(super) fn handle_agent_dialog_observe(
        &mut self,
        id: String,
        target: AgentTarget,
    ) -> String {
        self.reconcile_managed_agent_target(&target.target);
        let agent = match self.agent_info_for_target(&target.target) {
            Ok(agent) => agent,
            Err(error) => return encode_error_body(id, self.agent_target_error_body(error)),
        };
        let resolved = match self.resolve_agent_target(&target.target) {
            Ok(resolved) => resolved,
            Err(error) => return encode_error_body(id, self.agent_target_error_body(error)),
        };
        let Some(runtime) = self.lookup_runtime_sender(resolved.ws_idx, resolved.pane_id) else {
            return agent_not_found(id, &target.target);
        };
        let Some(observation) = dialog_observation(&agent, runtime) else {
            return encode_error(
                id,
                "dialog_observation_unavailable",
                "Agent screen changed while it was being observed",
            );
        };
        encode_success(id, ResponseResult::AgentDialog { observation })
    }

    /// Answers a choice dialog only while the agent's identity and the exact
    /// observed dialog still hold; otherwise nothing is sent.
    pub(super) fn handle_agent_dialog_choose(
        &mut self,
        id: String,
        params: AgentDialogChooseParams,
    ) -> String {
        self.reconcile_managed_agent_target(&params.target);
        let agent = match self.agent_info_for_target(&params.target) {
            Ok(agent) => agent,
            Err(error) => return encode_error_body(id, self.agent_target_error_body(error)),
        };
        let session = agent.agent_session.as_ref().map(|session| &session.value);
        if agent.terminal_id != params.expected_terminal_id
            || agent.pane_id != params.expected_pane_id
            || params
                .expected_session_id
                .as_ref()
                .is_some_and(|expected| session != Some(expected))
        {
            return encode_error(
                id,
                "agent_identity_changed",
                "Agent terminal, pane, or session identity changed; no keys were sent",
            );
        }
        let resolved = match self.resolve_agent_target(&params.target) {
            Ok(resolved) => resolved,
            Err(error) => return encode_error_body(id, self.agent_target_error_body(error)),
        };
        let Some(runtime) = self.lookup_runtime_sender(resolved.ws_idx, resolved.pane_id) else {
            return agent_not_found(id, &params.target);
        };
        let (keys, reason) = match runtime
            .try_choose_dialog_option(&params.expected_dialog_digest, params.option)
        {
            Ok(crate::pane::DialogChoice::Sent(keys)) => (keys, None),
            Ok(crate::pane::DialogChoice::Stale) => (Vec::new(), Some("stale_or_changed_dialog")),
            Ok(crate::pane::DialogChoice::Unreachable) => {
                (Vec::new(), Some("option_missing_or_selection_not_visible"))
            }
            Ok(crate::pane::DialogChoice::NotQuestion) => {
                (Vec::new(), Some("not_a_free_text_question"))
            }
            Err(error) => return encode_error(id, "dialog_write_failed", error),
        };
        let Some(observation) = dialog_observation(&agent, runtime) else {
            return encode_error(
                id,
                "dialog_observation_unavailable",
                "Agent screen changed while the result was being reported",
            );
        };
        encode_success(
            id,
            ResponseResult::AgentDialogChosen {
                choice: AgentDialogChooseResult {
                    written: reason.is_none(),
                    reason: reason.map(str::to_owned),
                    keys,
                    observation,
                },
            },
        )
    }

    pub(super) fn handle_agent_dialog_answer(
        &mut self,
        id: String,
        params: AgentDialogAnswerParams,
    ) -> String {
        if let Err(error) =
            AgentDialogAnswerParams::validate_answer(params.text.as_deref(), params.skip)
        {
            return encode_error(id, "invalid_params", error);
        }
        self.reconcile_managed_agent_target(&params.target);
        let agent = match self.agent_info_for_target(&params.target) {
            Ok(agent) => agent,
            Err(error) => return encode_error_body(id, self.agent_target_error_body(error)),
        };
        let session = agent.agent_session.as_ref().map(|session| &session.value);
        if agent.terminal_id != params.expected_terminal_id
            || agent.pane_id != params.expected_pane_id
            || params
                .expected_session_id
                .as_ref()
                .is_some_and(|expected| session != Some(expected))
        {
            return encode_error(
                id,
                "agent_identity_changed",
                "Agent terminal, pane, or session identity changed; no keys were sent",
            );
        }
        let resolved = match self.resolve_agent_target(&params.target) {
            Ok(resolved) => resolved,
            Err(error) => return encode_error_body(id, self.agent_target_error_body(error)),
        };
        let Some(runtime) = self.lookup_runtime_sender(resolved.ws_idx, resolved.pane_id) else {
            return agent_not_found(id, &params.target);
        };
        let (keys, reason) = match runtime.try_answer_dialog(
            &params.expected_dialog_digest,
            params.text,
            params.skip,
        ) {
            Ok(crate::pane::DialogChoice::Sent(keys)) => (keys, None),
            Ok(crate::pane::DialogChoice::Stale) => (Vec::new(), Some("stale_or_changed_dialog")),
            Ok(crate::pane::DialogChoice::NotQuestion) => {
                (Vec::new(), Some("not_a_free_text_question"))
            }
            Ok(crate::pane::DialogChoice::Unreachable) => {
                (Vec::new(), Some("question_input_unavailable"))
            }
            Err(error) => return encode_error(id, "dialog_write_failed", error),
        };
        let Some(observation) = dialog_observation(&agent, runtime) else {
            return encode_error(
                id,
                "dialog_observation_unavailable",
                "Agent screen changed while the result was being reported",
            );
        };
        encode_success(
            id,
            ResponseResult::AgentDialogChosen {
                choice: AgentDialogChooseResult {
                    written: reason.is_none(),
                    reason: reason.map(str::to_owned),
                    keys,
                    observation,
                },
            },
        )
    }

    pub(super) fn handle_agent_send_keys(
        &mut self,
        id: String,
        params: AgentSendKeysParams,
    ) -> String {
        let resolved = match self.resolve_agent_target(&params.target) {
            Ok(resolved) => resolved,
            Err(err) => return encode_error_body(id, self.agent_target_error_body(err)),
        };
        let Some(terminal_id) = self
            .state
            .workspaces
            .get(resolved.ws_idx)
            .and_then(|workspace| workspace.terminal_id(resolved.pane_id))
        else {
            return agent_not_found(id, &params.target);
        };
        let Some(expected_agent) = self
            .state
            .terminals
            .get(terminal_id)
            .and_then(|terminal| terminal.effective_known_agent())
        else {
            return agent_not_ready(id, &params.target);
        };
        let Some(runtime) = self.lookup_runtime_sender(resolved.ws_idx, resolved.pane_id) else {
            return agent_not_found(id, &params.target);
        };
        if !crate::server::terminals::agents::runtime_hosts_agent(runtime, expected_agent) {
            return agent_not_ready(id, &params.target);
        }
        let encoded = match super::input_encoding::encode_api_keys(runtime, &params.keys) {
            Ok(encoded) => encoded,
            Err(key) => {
                return encode_error(id, "invalid_key", format!("unsupported key {key}"));
            }
        };
        let bytes: Vec<u8> = encoded.into_iter().flatten().collect();
        if let Err(err) = runtime.try_send_bytes(Bytes::from(bytes)) {
            return encode_error(id, "agent_send_keys_failed", err.to_string());
        }

        encode_success(id, ResponseResult::Ok {})
    }
}

fn dialog_observation(
    agent: &crate::protocol::api::schema::AgentInfo,
    runtime: &crate::terminal::TerminalRuntime,
) -> Option<AgentDialogObservation> {
    let (screen, content_revision) = runtime.visible_ansi_snapshot_with_seq()?;
    Some(AgentDialogObservation {
        terminal_id: agent.terminal_id.clone(),
        pane_id: agent.pane_id.clone(),
        session_id: agent
            .agent_session
            .as_ref()
            .map(|session| session.value.clone()),
        content_revision,
        dialog: crate::detect::dialog::parse(&screen).map(|dialog| AgentDialog {
            kind: match dialog.kind {
                crate::detect::dialog::DialogKind::Choice => AgentDialogKind::Choice,
                crate::detect::dialog::DialogKind::Question => AgentDialogKind::Question,
            },
            id: dialog.id(),
            digest: dialog.digest(),
            text: dialog.text,
            options: dialog
                .options
                .into_iter()
                .map(|option| AgentDialogOption {
                    number: option.number,
                    label: option.label,
                    selected: option.selected,
                })
                .collect(),
            hint: dialog.hint,
        }),
    })
}

fn agent_not_ready(id: String, target: &str) -> String {
    encode_error(
        id,
        "agent_not_ready",
        format!("agent {target} is not an active named agent"),
    )
}

fn agent_not_found(id: String, target: &str) -> String {
    encode_error(
        id,
        "agent_not_found",
        format!("agent target {target} not found"),
    )
}

#[cfg(test)]
#[path = "tests/basic_test.rs"]
mod tests;
