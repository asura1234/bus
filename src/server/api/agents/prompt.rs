//! Agent prompt API operations.
use super::{agent_not_found, agent_not_ready};
use crate::protocol::api::schema::{AgentPromptParams, ResponseResult};
use crate::server::api::errors::{encode_error, encode_error_body, encode_success};
use crate::server::app::App;
use crate::terminal::pty::actor::InputCompletion;
use bytes::Bytes;
use std::time::Duration;

mod composer;
mod grok;
mod kimi;

#[cfg(test)]
pub(super) use crate::protocol::api::submission::SUBMIT_DELAY as AGENT_PROMPT_SUBMIT_DELAY;

pub(super) fn detection_composer(
    kind: crate::agents::AgentKind,
) -> Option<Box<dyn crate::terminal::runtime::SubmissionInspector>> {
    observes_composer(kind).then(|| {
        Box::new(composer::observer(kind, String::new()))
            as Box<dyn crate::terminal::runtime::SubmissionInspector>
    })
}

fn observes_composer(kind: crate::agents::AgentKind) -> bool {
    use crate::agents::AgentKind;
    matches!(
        kind,
        AgentKind::Codex
            | AgentKind::Claude
            | AgentKind::Cursor
            | AgentKind::Grok
            | AgentKind::Kimi
    )
}

struct StyledChar {
    character: char,
    faint: bool,
    background: bool,
}

/// Retain the styles that distinguish suggestions and composer chrome.
fn styled_chars(line: &str) -> Vec<StyledChar> {
    let mut out = Vec::new();
    let (mut dim, mut inverse, mut background) = (false, false, false);
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(StyledChar {
                character: c,
                faint: dim || inverse,
                background,
            });
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
                    apply_sgr(&params, &mut dim, &mut inverse, &mut background);
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

fn plain_screen(screen: &str) -> String {
    screen
        .lines()
        .map(|line| {
            styled_chars(line)
                .iter()
                .map(|c| c.character)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn apply_sgr(params: &str, dim: &mut bool, inverse: &mut bool, background: &mut bool) {
    let params: Vec<&str> = params.split(';').collect();
    let mut index = 0;
    while index < params.len() {
        match params[index] {
            "" | "0" => (*dim, *inverse, *background) = (false, false, false),
            "2" => *dim = true,
            "22" => *dim = false,
            "7" => *inverse = true,
            "27" => *inverse = false,
            "49" => *background = false,
            // Extended colors carry their own numbers (38;2;R;G;B or 38;5;N),
            // which must not read as dim or inverse.
            "38" | "48" | "58" => {
                if params[index] == "48" {
                    *background = true;
                }
                index += match params.get(index + 1) {
                    Some(&"5") => 2,
                    Some(&"2") => 4,
                    _ => 0,
                }
            }
            value
                if value.parse::<u8>().ok().is_some_and(|value| {
                    (40..=47).contains(&value) || (100..=107).contains(&value)
                }) =>
            {
                *background = true;
            }
            _ => {}
        }
        index += 1;
    }
}

pub(super) fn check_unbound_prompt_identity_and_idle(
    agent: &crate::protocol::api::schema::AgentInfo,
    params: &crate::protocol::api::schema::AgentPromptIfUnboundParams,
) -> Result<(), (&'static str, &'static str)> {
    if agent.terminal_id != params.expected_terminal_id
        || agent.pane_id != params.expected_pane_id
        || !matches!(agent.agent.as_deref(), Some("codex" | "kimi"))
        || agent.name.as_deref() != Some(params.expected_managed_name.as_str())
        || params.expected_managed_name.is_empty()
        || agent.agent_session.is_some()
    {
        return Err((
            "agent_identity_changed",
            "Managed launch identity changed; prompt was not sent",
        ));
    }
    // A visible dialog takes typed text as its answer, as on a bound agent.
    if !matches!(
        agent.agent_status,
        crate::protocol::api::schema::AgentStatus::Idle
            | crate::protocol::api::schema::AgentStatus::Done
    ) || agent.dialog_id.is_some()
    {
        return Err(("agent_not_idle", "Agent is not idle; prompt was not sent"));
    }
    if agent.launch_pending || !agent.interactive_ready {
        return Err(("agent_not_ready", "Agent is not ready; prompt was not sent"));
    }
    Ok(())
}

pub(super) fn check_prompt_identity_and_idle(
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

pub(super) fn agent_prompt_submit_delay(
    agent: crate::agents::AgentKind,
    prompt_bytes: usize,
) -> Duration {
    crate::protocol::api::submission::submit_delay(
        agent == crate::agents::AgentKind::Codex,
        prompt_bytes,
    )
}

impl App {
    pub(in crate::server) fn handle_deferred_agent_api_message(
        &mut self,
        message: crate::server::api::ApiRequestMessage,
    ) -> bool {
        self.handle_deferred_agent_api_request_at(
            message.request,
            message.respond_to,
            message.received_at,
        )
    }

    #[cfg(test)]
    pub(crate) fn handle_deferred_agent_api_request(
        &mut self,
        request: crate::protocol::api::schema::Request,
        respond_to: std::sync::mpsc::Sender<String>,
    ) -> bool {
        let started = self
            .test_prompt_clock(&request.method)
            .map_or_else(std::time::Instant::now, |clock| clock.now());
        self.handle_deferred_agent_api_request_at(request, respond_to, started)
    }

    pub(super) fn handle_deferred_agent_api_request_at(
        &mut self,
        request: crate::protocol::api::schema::Request,
        respond_to: std::sync::mpsc::Sender<String>,
        started: std::time::Instant,
    ) -> bool {
        let Some(budget) = crate::protocol::api::submission::method_budget(&request.method) else {
            return false;
        };
        let hard_deadline = started + budget;
        #[cfg(test)]
        let test_clock = self.test_prompt_clock(&request.method);
        let now = std::time::Instant::now();
        #[cfg(test)]
        let now = test_clock.as_ref().map_or(now, |clock| clock.now());
        if now >= hard_deadline {
            // No lookup, paste or Enter has run. Queue latency cannot turn
            // definite non-delivery into a fresh native submission window.
            let _ = respond_to.send(encode_error(
                request.id,
                crate::protocol::api::AGENT_PROMPT_NOT_SHOWN,
                crate::protocol::api::AGENT_PROMPT_NOT_SHOWN_MESSAGE,
            ));
            return true;
        }
        let api_request_id = request.id.clone();
        let queued = match request.method {
            crate::protocol::api::schema::Method::AgentPrompt(params) => {
                self.queue_agent_prompt(request.id, params, false, hard_deadline, false)
            }
            crate::protocol::api::schema::Method::AgentPromptIfIdle(params) => {
                self.queue_agent_prompt_if_idle(request.id, params, hard_deadline)
            }
            crate::protocol::api::schema::Method::AgentPromptIfUnbound(params) => {
                self.queue_agent_prompt_if_unbound(request.id, params, hard_deadline)
            }
            _ => return false,
        };
        match queued {
            Ok((id, agent, completion)) => {
                tracing::debug!(target: "bus::server::api::agents", event = "bus.terminal.queued", api_request_id = %id,
                    pane_id = %agent.pane_id, terminal_id = %agent.terminal_id,
                    "Prompt submission queued in native terminal writer");
                std::thread::spawn(move || {
                    // A blocked native writer must not extend the RPC forever.
                    // Timeout stays uncertain: Enter may already have been sent.
                    let now = std::time::Instant::now();
                    #[cfg(test)]
                    let now = test_clock.as_ref().map_or(now, |clock| clock.now());
                    #[cfg(test)]
                    let map_timeout = |timeout| {
                        test_clock
                            .as_ref()
                            .map_or(timeout, |clock| clock.wait_timeout(timeout))
                    };
                    #[cfg(not(test))]
                    let map_timeout = std::convert::identity;
                    let response = complete_agent_prompt_submission(
                        id,
                        agent,
                        hard_deadline,
                        started,
                        now,
                        completion,
                        map_timeout,
                    );
                    let _ = respond_to.send(response);
                });
            }
            Err(response) => {
                tracing::info!(target: "bus::server::api::agents", event = "bus.terminal.rejected", api_request_id = %api_request_id,
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
        hard_deadline: std::time::Instant,
    ) -> Result<
        (
            String,
            crate::protocol::api::schema::AgentInfo,
            InputCompletion,
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
            true,
            hard_deadline,
            false,
        )
    }

    fn queue_agent_prompt_if_idle(
        &mut self,
        id: String,
        params: crate::protocol::api::schema::AgentPromptIfIdleParams,
        hard_deadline: std::time::Instant,
    ) -> Result<
        (
            String,
            crate::protocol::api::schema::AgentInfo,
            InputCompletion,
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
        self.queue_agent_prompt(
            id,
            AgentPromptParams {
                target: params.target,
                text: params.text,
                wait: None,
            },
            true,
            hard_deadline,
            params.steer,
        )
    }

    fn queue_agent_prompt(
        &mut self,
        id: String,
        params: AgentPromptParams,
        observe: bool,
        hard_deadline: std::time::Instant,
        steer: bool,
    ) -> Result<
        (
            String,
            crate::protocol::api::schema::AgentInfo,
            InputCompletion,
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
        if terminal.state == crate::agents::AgentState::Blocked {
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
        if terminal.managed_agent_awaiting_startup() {
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
        let caller_deadline = params
            .wait
            .as_ref()
            .and_then(|wait| wait.submission_deadline);
        #[cfg(not(windows))]
        let caller_deadline = None;
        let submit_deadline = Some(
            caller_deadline.map_or(hard_deadline, |deadline: std::time::Instant| {
                deadline.min(hard_deadline)
            }),
        );
        if expected_agent == crate::agents::AgentKind::GithubCopilot {
            // Copilot ignores synthetic Enter after focus loss until it receives focus gained.
            let focus =
                match crate::terminal::vt::encode_focus(crate::terminal::vt::FocusEvent::Gained) {
                    Ok(focus) => focus,
                    Err(err) => {
                        return Err(encode_error(id, "agent_prompt_failed", err.to_string()));
                    }
                };
            if let Err(err) = runtime.try_send_bytes(Bytes::from(focus)) {
                return Err(encode_error(id, "agent_prompt_failed", err.to_string()));
            }
        }
        let Some(agent) = self.agent_info(resolved.ws_idx, resolved.pane_id) else {
            return Err(agent_not_found(id, &params.target));
        };
        let span = tracing::info_span!(target: "bus::terminal::submission", "native_submission", api_request_id = %id, pane_id = %agent.pane_id, terminal_id = %agent.terminal_id, provider = crate::agents::agent_label(expected_agent));
        let _entered = span.enter();
        let completion = queue_prompt_input(
            runtime,
            expected_agent,
            params.text,
            observe,
            submit_delay,
            submit_deadline,
            Some(crate::platform::draft_editor::directory(&agent.pane_id)),
            steer,
        )
        .map_err(|err| encode_error(id.clone(), "agent_prompt_failed", err.to_string()))?;
        Ok((id, agent, completion))
    }
}

fn queue_prompt_input(
    runtime: &crate::terminal::TerminalRuntime,
    kind: crate::agents::AgentKind,
    text: String,
    observe: bool,
    delay: Duration,
    deadline: Option<std::time::Instant>,
    draft_directory: Option<std::path::PathBuf>,
    steer: bool,
) -> std::io::Result<InputCompletion> {
    use crate::agents::AgentKind;
    let (mut encoded, enter) =
        crate::server::api::input_encoding::encode_api_submission_parts(runtime, &text);
    if kind == AgentKind::Codex && runtime.bracketed_paste_enabled() {
        let (images, remaining) = crate::agents::providers::codex::hooks::image_paste_parts(&text);
        if !images.is_empty() {
            // Each image must arrive as a standalone paste event. Keep all
            // parts inside this submission's ownership/cancellation boundary;
            // the inspector still owns the complete logical prompt and Enter.
            encoded.clear();
            for part in images
                .into_iter()
                .chain((!remaining.is_empty()).then_some(remaining.as_str()))
            {
                encoded.extend(crate::server::api::input_encoding::encode_api_text(
                    runtime, part,
                ));
            }
        }
    }
    if !observe || !observes_composer(kind) {
        return runtime.queue_user_input_submission(
            Bytes::from(encoded),
            Bytes::from(enter),
            delay,
            deadline,
        );
    }
    let clear = crate::server::api::input_encoding::encode_api_keys(
        runtime,
        &["ctrl+a".into(), "ctrl+k".into(), "ctrl+u".into()],
    )
    .map_err(|key| std::io::Error::other(format!("Invalid clear key: {key}")))?;
    let inspector: Box<dyn crate::terminal::runtime::SubmissionInspector> =
        if let Some(directory) = draft_directory {
            let keys = crate::server::api::input_encoding::encode_api_keys(
                runtime,
                &["ctrl+s".into(), "ctrl+g".into(), "backspace".into()],
            )
            .map_err(|key| std::io::Error::other(format!("Invalid draft key: {key}")))?;
            Box::new(composer::preserving_observer_for_submission(
                kind,
                text,
                directory,
                keys.into_iter().map(Bytes::from).collect(),
                steer,
            ))
        } else {
            Box::new(composer::observer(kind, text))
        };
    runtime.queue_observed_input_with_inspector(
        Bytes::from(encoded),
        Bytes::from(enter),
        clear.into_iter().map(Bytes::from).collect(),
        delay,
        deadline,
        inspector,
    )
}

/// Keep cancellation owned until the response has been formatted, including
/// the bounded extra Enter delay of an admitted combined submission.
pub(super) fn complete_agent_prompt_submission(
    id: String,
    agent: crate::protocol::api::schema::AgentInfo,
    hard_deadline: std::time::Instant,
    started: std::time::Instant,
    now: std::time::Instant,
    completion: InputCompletion,
    map_timeout: impl FnOnce(Duration) -> Duration,
) -> String {
    let completion_deadline = hard_deadline + completion.enter_delay_allowance();
    complete_agent_prompt(id, agent, completion_deadline, started, now, |timeout| {
        Ok(completion.wait_for_api(map_timeout(timeout)))
    })
}

// The waiter is injectable so boundary races use a virtual response timeline.
pub(super) fn complete_agent_prompt(
    id: String,
    agent: crate::protocol::api::schema::AgentInfo,
    hard_deadline: std::time::Instant,
    started: std::time::Instant,
    now: std::time::Instant,
    receive: impl FnOnce(Duration) -> Result<std::io::Result<()>, std::sync::mpsc::RecvTimeoutError>,
) -> String {
    let response_deadline = hard_deadline + crate::protocol::api::submission::RESPONSE_GRACE;
    let completed = receive(response_deadline.saturating_duration_since(now));
    let not_shown = |err: &std::io::Error| crate::terminal::runtime::PromptNotShown::is(err);
    let outcome = match &completed {
        Ok(Ok(())) => "written",
        Ok(Err(err)) if not_shown(err) => "not_shown",
        Ok(Err(err)) if err.kind() == std::io::ErrorKind::TimedOut => "timeout",
        Ok(Err(_)) => "write_failed",
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => "timeout",
        Err(_) => "writer_closed",
    };
    tracing::info!(target: "bus::server::api::agents", event = "bus.terminal.result", api_request_id = %id,
        pane_id = %agent.pane_id, terminal_id = %agent.terminal_id, outcome,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "Native prompt submission completed; this is not model acceptance");
    match completed {
        Ok(Ok(())) => encode_success(id, ResponseResult::AgentPrompted { agent }),
        Ok(Err(err)) if crate::terminal::runtime::PromptNotShown::composer_busy(&err) => {
            encode_error(
                id,
                crate::protocol::api::AGENT_COMPOSER_BUSY,
                err.to_string(),
            )
        }
        Ok(Err(err)) if not_shown(&err) => encode_error(
            id,
            crate::protocol::api::AGENT_PROMPT_NOT_SHOWN,
            err.to_string(),
        ),
        Ok(Err(err)) if err.kind() == std::io::ErrorKind::TimedOut => {
            encode_error(id, "timeout", err.to_string())
        }
        Ok(Err(err)) => encode_error(id, "agent_prompt_failed", err.to_string()),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => encode_error(
            id,
            "timeout",
            "Native prompt submission exceeded its hard deadline",
        ),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            encode_error(id, "agent_prompt_failed", "pty actor closed")
        }
    }
}
