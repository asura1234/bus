//! Agent dialog API operations.
use super::agent_not_found;
use crate::protocol::api::schema::{
    AgentDialog, AgentDialogAnswerParams, AgentDialogChooseParams, AgentDialogChooseResult,
    AgentDialogKind, AgentDialogObservation, AgentDialogOption, AgentTarget, ResponseResult,
};
use crate::server::api::errors::{encode_error, encode_error_body, encode_success};
use crate::server::app::App;

impl App {
    pub(in crate::server::api) fn handle_agent_dialog_observe(
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
        let (keys, reason) =
            match runtime.try_choose_dialog_option(&params.expected_dialog_digest, params.option) {
                Ok(crate::terminal::runtime::DialogChoice::Sent(keys)) => (keys, None),
                Ok(crate::terminal::runtime::DialogChoice::Stale) => {
                    (Vec::new(), Some("stale_or_changed_dialog"))
                }
                Ok(crate::terminal::runtime::DialogChoice::Unreachable) => {
                    (Vec::new(), Some("option_missing_or_selection_not_visible"))
                }
                Ok(crate::terminal::runtime::DialogChoice::NotQuestion) => {
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
            Ok(crate::terminal::runtime::DialogChoice::Stale) => {
                (Vec::new(), Some("stale_or_changed_dialog"))
            }
            Ok(crate::terminal::runtime::DialogChoice::NotQuestion) => {
                (Vec::new(), Some("not_a_free_text_question"))
            }
            Ok(crate::terminal::runtime::DialogChoice::Unreachable) => {
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
        dialog: crate::agents::dialog::parse(&screen).map(|dialog| AgentDialog {
            kind: match dialog.kind {
                crate::agents::dialog::DialogKind::Choice => AgentDialogKind::Choice,
                crate::agents::dialog::DialogKind::Question => AgentDialogKind::Question,
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
