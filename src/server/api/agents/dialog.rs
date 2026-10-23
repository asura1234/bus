//! Agent dialog API operations.
use super::agent_not_found;
use crate::protocol::api::schema::{
    AgentDialog, AgentDialogAnswerParams, AgentDialogChooseParams, AgentDialogChooseResult,
    AgentDialogKind, AgentDialogObservation, AgentDialogObserveParams, AgentDialogOption,
    ResponseResult,
};
use crate::server::api::errors::{encode_error, encode_error_body, encode_success};
use crate::server::app::App;
use sha2::{Digest, Sha256};

/// A written answer that only moved to a hidden free-text row; the caller
/// checks the redrawn field before typing.
pub(crate) const FREE_TEXT_FOCUSED: &str = "free_text_focused";

impl App {
    pub(in crate::server::api) fn handle_agent_dialog_observe(
        &mut self,
        id: String,
        target: AgentDialogObserveParams,
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
        if target.open_pending_question && agent.agent.as_deref() == Some("codex") {
            if let Err(error) = runtime.try_open_pending_codex_question() {
                return encode_error(id, "dialog_write_failed", error);
            }
        }
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
    pub(in crate::server::api) fn handle_agent_dialog_choose(
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
        let choice =
            if params.action == crate::protocol::api::schema::AgentDialogChoiceAction::Choose {
                runtime.try_choose_dialog_option(&params.expected_dialog_digest, params.option)
            } else {
                runtime.try_control_dialog_option(
                    &params.expected_dialog_digest,
                    params.option,
                    params.action,
                )
            };
        let (keys, reason) = match choice {
            Ok(crate::terminal::runtime::DialogChoice::Sent(keys)) => (keys, None),
            Ok(crate::terminal::runtime::DialogChoice::Redrawing) => {
                return encode_error(
                    id,
                    "dialog_redrawing",
                    "No keys were sent; dialog is redrawing; observe it again",
                );
            }
            Ok(crate::terminal::runtime::DialogChoice::Stale) => {
                (Vec::new(), Some("stale_or_changed_dialog"))
            }
            Ok(crate::terminal::runtime::DialogChoice::Unreachable) => {
                (Vec::new(), Some("option_missing_or_selection_not_visible"))
            }
            Ok(
                crate::terminal::runtime::DialogChoice::NotQuestion
                | crate::terminal::runtime::DialogChoice::Unskippable
                | crate::terminal::runtime::DialogChoice::FreeTextDraft
                | crate::terminal::runtime::DialogChoice::Focused(_),
            ) => (Vec::new(), Some("not_a_free_text_question")),
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

    pub(in crate::server::api) fn handle_agent_dialog_answer(
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
            Ok(crate::terminal::runtime::DialogChoice::Sent(keys)) => (keys, None),
            Ok(crate::terminal::runtime::DialogChoice::Redrawing) => {
                return encode_error(
                    id,
                    "dialog_redrawing",
                    "No keys were sent; dialog is redrawing; observe it again",
                );
            }
            Ok(crate::terminal::runtime::DialogChoice::Stale) => {
                (Vec::new(), Some("stale_or_changed_dialog"))
            }
            Ok(crate::terminal::runtime::DialogChoice::NotQuestion) => {
                (Vec::new(), Some("not_a_free_text_question"))
            }
            Ok(crate::terminal::runtime::DialogChoice::Unreachable) => {
                (Vec::new(), Some("question_input_unavailable"))
            }
            Ok(crate::terminal::runtime::DialogChoice::Unskippable) => {
                (Vec::new(), Some("skip_would_cancel_form"))
            }
            Ok(crate::terminal::runtime::DialogChoice::FreeTextDraft) => {
                (Vec::new(), Some("free_text_holds_draft"))
            }
            Ok(crate::terminal::runtime::DialogChoice::Focused(keys)) => {
                (keys, Some(FREE_TEXT_FOCUSED))
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
                    written: reason.is_none_or(|reason| reason == FREE_TEXT_FOCUSED),
                    reason: reason.map(str::to_owned),
                    keys,
                    observation,
                },
            },
        )
    }
}

fn dialog_observation(
    agent: &crate::protocol::api::schema::AgentInfo,
    runtime: &crate::terminal::TerminalRuntime,
) -> Option<AgentDialogObservation> {
    let started = std::time::Instant::now();
    let (screen, content_revision) = runtime.visible_ansi_snapshot_with_seq()?;
    let pending_question = agent.agent.as_deref() == Some("codex")
        && crate::agents::dialog::codex_question_pending(&screen);
    let dialog = crate::agents::dialog::parse(&screen);
    runtime.trace_dialog_fingerprint(dialog.as_ref(), "observe", None, content_revision, false);
    tracing::debug!(
        event = "bus.dialog.observed",
        terminal_id = %agent.terminal_id,
        pane_id = %agent.pane_id,
        content_revision,
        status = ?agent.agent_status,
        listed_dialog_id = ?agent.dialog_id,
        parsed_dialog_id = ?dialog.as_ref().map(|dialog| dialog.id()),
        dialog_kind = ?dialog.as_ref().map(|dialog| dialog.kind),
        option_count = dialog.as_ref().map_or(0, |dialog| dialog.options.len()),
        elapsed_us = started.elapsed().as_micros() as u64,
        pending_question,
        synchronized_output = runtime.synchronized_output_active(),
        "Parsed the current native dialog snapshot"
    );
    if dialog.is_none()
        && !pending_question
        && (agent.dialog_id.is_some()
            || agent.agent_status == crate::protocol::api::schema::AgentStatus::Blocked)
    {
        // Correlate snapshots without retaining prompts, commands or output.
        tracing::trace!(event = "bus.dialog.missed_blocker", terminal_id = %agent.terminal_id,
            pane_id = %agent.pane_id, content_revision,
            screen_hash = %format!("{:x}", Sha256::digest(screen.as_bytes())),
            screen_bytes = screen.len(),
            "Blocked metadata has no readable dialog in this snapshot");
    }
    Some(AgentDialogObservation {
        pending_question,
        terminal_id: agent.terminal_id.clone(),
        pane_id: agent.pane_id.clone(),
        session_id: agent
            .agent_session
            .as_ref()
            .map(|session| session.value.clone()),
        content_revision,
        dialog: dialog.map(|dialog| AgentDialog {
            kind: match dialog.kind {
                crate::agents::dialog::DialogKind::Choice => AgentDialogKind::Choice,
                crate::agents::dialog::DialogKind::Question => AgentDialogKind::Question,
            },
            id: dialog.id(),
            digest: dialog.digest(),
            free_text: dialog.free_text_summary().map(|(option, draft, checkbox)| {
                crate::protocol::api::schema::AgentDialogFreeText {
                    option,
                    draft,
                    checkbox,
                }
            }),
            tabs: dialog
                .tabs
                .into_iter()
                .map(|tab| crate::protocol::api::schema::AgentDialogTab {
                    label: tab.label,
                    answered: tab.answered,
                    active: tab.active,
                })
                .collect(),
            text: dialog.text,
            options: dialog
                .options
                .into_iter()
                .map(|option| AgentDialogOption {
                    number: option.number,
                    label: option.label,
                    selected: option.selected,
                    free_text: option.free_text,
                })
                .collect(),
            hint: dialog.hint,
        }),
    })
}
