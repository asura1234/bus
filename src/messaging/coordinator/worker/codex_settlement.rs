//! Read Codex terminal evidence before settling a request.
use crate::messaging::model::{AgentId, BusState, Provider};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub(super) fn saved_rollouts(state: &BusState) -> BTreeMap<AgentId, PathBuf> {
    state
        .agents()
        .filter_map(|agent| {
            agent
                .codex_rollout
                .clone()
                .map(|rollout| (agent.id, rollout))
        })
        .collect()
}

/// How Codex recorded the end of `request`'s turn in its rollout: the bound
/// turn, or else the one turn that ran the typed prompt after it was sent.
/// Why nothing could be read or attributed is logged, since the request then
/// closes as `reply_not_captured`.
pub(super) fn recorded_codex_end(
    rollout: &Path,
    request: &crate::messaging::model::Request,
) -> Option<crate::messaging::model::RecordedTurnEnd> {
    use crate::agents::providers::codex::usage::{
        codex_prompt_turn, codex_turn_prompts, read_codex_turn_end, CodexTurnEnd, PromptTurn,
    };
    use crate::messaging::model::RecordedTurnEnd;
    let unreadable = |error: &std::io::Error| {
        tracing::warn!(
            event = "bus.message.rollout_unreadable",
            request_id = request.id.0,
            agent_id = request.agent_id.0,
            error = %error,
            "Codex rollout could not be read for the turn's reply"
        );
    };
    let sent_ms = request.prompt.submitted_at_ms;
    let turn_id = match &request.provider_turn_id {
        Some(turn) => turn.clone(),
        None => match codex_prompt_turn(rollout, sent_ms, |prompt| {
            request.matches_callback_payload(prompt, Provider::Codex)
        }) {
            Ok(PromptTurn::Found(turn)) => turn,
            Ok(outcome) => {
                tracing::info!(
                    event = "bus.message.rollout_unattributed",
                    request_id = request.id.0,
                    agent_id = request.agent_id.0,
                    outcome = ?outcome,
                    "No rollout turn provably ran this message"
                );
                return None;
            }
            Err(error) => {
                unreadable(&error);
                return None;
            }
        },
    };
    let end = match read_codex_turn_end(rollout, &turn_id) {
        Ok(end) => end?,
        Err(error) => {
            unreadable(&error);
            return None;
        }
    };
    Some(match end {
        CodexTurnEnd::Error(message) => RecordedTurnEnd::Error(message),
        CodexTurnEnd::Final(text) => {
            let prompts = codex_turn_prompts(rollout, &turn_id, sent_ms)
                .inspect_err(unreadable)
                .unwrap_or_default();
            RecordedTurnEnd::Final {
                turn_id,
                text,
                prompts,
            }
        }
    })
}
