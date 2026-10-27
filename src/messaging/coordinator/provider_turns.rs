//! Provider turn facts read beyond the hook's own verdict: background shells
//! for wake ownership, and Codex usage from its rollout.
use super::{callbacks, AgentId, Parsed, Path, Provider, ProviderCallback, Worker};

impl Worker {
    pub(super) fn mark_scheduled_turn(
        state: &mut super::BusState,
        callback: &ProviderCallback,
        record: &callbacks::Record,
    ) -> bool {
        if record.manifest.provider != Provider::ClaudeCode
            || !matches!(callback.kind, super::CallbackEventKind::PromptStarted)
        {
            return true;
        }
        use crate::agents::providers::claude_code::hooks::{prompt_origin, PromptOrigin};
        match prompt_origin(&record.value) {
            Some(PromptOrigin::Scheduled | PromptOrigin::Human) => {
                // Explicit input Bus submitted still wins inside this model guard.
                state.mark_unowned_background_turn(callback);
            }
            None if record
                .value
                .get("transcript_path")
                .and_then(serde_json::Value::as_str)
                .is_some()
                && state
                    .agent(callback.agent_id)
                    .and_then(|agent| agent.current_request)
                    .and_then(|id| state.request(id))
                    .is_some_and(|request| request.awaiting_background) =>
            {
                // Keep the hook spooled: an unpublished row is not evidence of
                // an owned continuation without a captured answer.
                tracing::debug!(
                    event = "bus.callback.deferred",
                    reason = "claude_prompt_origin_pending",
                    "Awaiting Claude prompt origin before continuing a textless background pause"
                );
                return false;
            }
            _ => {}
        }
        true
    }

    /// Observe shell ownership for rejecting inherited wake notifications.
    /// A captured Stop answer is final regardless of these shells.
    pub(super) fn observe_background_shells(
        state: &mut super::BusState,
        id: AgentId,
        record: &callbacks::Record,
        parsed: &mut Parsed,
    ) -> Result<(), String> {
        if record.manifest.provider != Provider::ClaudeCode {
            return Ok(());
        }
        let running = matches!(
            parsed,
            Parsed::Final { .. } | Parsed::BackgroundPending { .. }
        )
        .then(|| {
            crate::agents::providers::claude_code::hooks::claude_running_shells(&record.value)
        });
        let owned = state
            .observe_claude_shells(id, running)
            .map_err(|e| e.to_string())?;
        if let Parsed::Final {
            session,
            turn,
            text,
        } = parsed
        {
            if text.trim().is_empty() && owned {
                *parsed = Parsed::BackgroundPending {
                    session: session.clone(),
                    turn: turn.clone(),
                };
            }
        }
        Ok(())
    }

    pub(super) fn is_inherited_shell_wake(
        state: &super::BusState,
        id: AgentId,
        callback: &ProviderCallback,
    ) -> bool {
        let Some(inventory) = state
            .agent(id)
            .and_then(|agent| agent.claude_shells.as_ref())
        else {
            return false;
        };
        let Some(body) = callback
            .prompt_payload
            .as_deref()
            .and_then(crate::agents::providers::claude_code::hooks::task_notification_body)
        else {
            return false;
        };
        let task = body
            .trim()
            .strip_prefix("<task-id>")
            .and_then(|tagged| tagged.split_once("</task-id>").map(|(task, _)| task.trim()));
        task.is_some_and(|task| inventory.inherited.contains(task))
    }

    /// Codex writes rate limits into its rollout after each turn. Usage is
    /// advisory: a failed read keeps the previous snapshot and never errors.
    pub(in crate::messaging::coordinator) fn refresh_codex_usage(
        &mut self,
        id: AgentId,
        path: &Path,
    ) {
        match crate::agents::providers::codex::usage::read_codex_observation(path) {
            Ok(Some(observation)) => {
                if observation.observed_at_ms > crate::utils::time::now_ms()
                    || self
                        .usage
                        .codex
                        .as_ref()
                        .is_some_and(|old| old.read_at_ms >= observation.observed_at_ms)
                {
                    return;
                }
                self.usage.codex = Some(crate::messaging::coordinator::usage::UsageSnapshot {
                    windows: observation.windows,
                    read_at_ms: observation.observed_at_ms,
                    observed_by_agent: id,
                });
            }
            Ok(None) => tracing::debug!(
                event = "bus.usage.unavailable",
                agent_id = id.0,
                "Codex rollout has no rate limits yet"
            ),
            Err(error) => tracing::debug!(
                event = "bus.usage.unavailable",
                agent_id = id.0,
                %error,
                "Codex rollout unreadable"
            ),
        }
    }
}
