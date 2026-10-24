//! Provider turn facts read beyond the hook's own verdict: background shells
//! a Claude turn is still waiting on, and Codex usage from its rollout.
use super::{callbacks, AgentId, BTreeSet, Parsed, Path, Provider, ProviderCallback, Worker};

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
                // an owned continuation. The existing hold cap still runs.
                tracing::debug!(
                    event = "bus.callback.deferred",
                    reason = "claude_prompt_origin_pending",
                    "Awaiting Claude prompt origin before continuing a background hold"
                );
                return false;
            }
            _ => {}
        }
        true
    }

    /// A Claude reply waits while shells started by its request still run:
    /// each wakes a turn when it ends, and the last such turn carries the real
    /// reply. Shells inherited from an earlier request (say a hung
    /// `find /`) are listed by every later `Stop` and never hold a reply.
    /// Every held `Stop`, including cron/non-shell waits, returns its interim
    /// text for bounded delivery. Missing text remains an observation only.
    pub(super) fn hold_background_reply(
        &mut self,
        id: AgentId,
        record: &callbacks::Record,
        parsed: Parsed,
    ) -> (Parsed, Option<String>) {
        if record.manifest.provider != Provider::ClaudeCode {
            return (parsed, None);
        }
        // A freshly started Claude process has no shells running.
        if matches!(&parsed, Parsed::Session { source: Some(source), .. } if source == "startup") {
            self.claude_settled_shells.insert(id, BTreeSet::new());
            self.claude_shell_observations.remove(&id);
        }
        self.inherit_shells_for_current_request(id);
        // The parser reports cron/non-shell waits before shell classification.
        // Keep that Stop's text too, otherwise it bypasses the persisted hold
        // and its deadline forever, even when shells are also running.
        if matches!(&parsed, Parsed::BackgroundPending { .. }) {
            let interim = background_stop_reply(id, record);
            return (parsed, interim);
        }
        let Parsed::Final {
            session,
            turn,
            text,
        } = parsed
        else {
            return (parsed, None);
        };
        let running =
            crate::agents::providers::claude_code::hooks::claude_running_shells(&record.value);
        let request = self.state.agent(id).and_then(|agent| agent.current_request);
        if let Some((_, observed)) = self.claude_shell_observations.get_mut(&id) {
            observed.clone_from(&running);
        }
        // A previous request's shell report must not hold or settle this
        // request, even while this request waits on a different own shell.
        if self
            .state
            .is_unrelated_provider_turn(&record.manifest.launch_id, &session, &turn)
        {
            return (
                Parsed::Final {
                    session,
                    turn,
                    text,
                },
                None,
            );
        }
        let has_request = request.is_some();
        let started: Vec<_> = match self.claude_settled_shells.get(&id) {
            Some(settled) => running.difference(settled).cloned().collect(),
            // Unknown after a restart: treat every running shell as older.
            None => Vec::new(),
        };
        if has_request && !started.is_empty() {
            let held_request = request.and_then(|request| self.state.request(request));
            tracing::info!(
                event = "bus.callback.background_shells",
                agent_id = id.0,
                request_id = request.map(|request| request.0),
                prompt_id = held_request.map(|request| request.prompt.id.0),
                author = held_request.map(|request| match request.prompt.author {
                    crate::messaging::model::Author::Developer => "developer",
                    crate::messaging::model::Author::Agent(_) => "agent",
                    crate::messaging::model::Author::Bus => "bus",
                }),
                captured_reply_bytes = text.len(),
                inherited_shell_count = running.len().saturating_sub(started.len()),
                started_shell_count = started.len(),
                shells = ?started,
                "Reply waits for shells this request started"
            );
            let interim = (!text.trim().is_empty()).then_some(text);
            return (Parsed::BackgroundPending { session, turn }, interim);
        }
        self.claude_settled_shells.insert(id, running);
        (
            Parsed::Final {
                session,
                turn,
                text,
            },
            None,
        )
    }

    fn inherit_shells_for_current_request(&mut self, id: AgentId) {
        let request = self.state.agent(id).and_then(|agent| agent.current_request);
        match self.claude_shell_observations.get_mut(&id) {
            Some((owner, running)) if *owner != request => {
                self.claude_settled_shells.insert(id, running.clone());
                *owner = request;
            }
            Some(_) => {}
            None => {
                self.claude_shell_observations
                    .insert(id, (request, BTreeSet::new()));
            }
        }
    }

    pub(super) fn is_inherited_shell_wake(&self, id: AgentId, callback: &ProviderCallback) -> bool {
        let Some(inherited) = self.claude_settled_shells.get(&id) else {
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
        task.is_some_and(|task| inherited.contains(task))
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

fn background_stop_reply(id: AgentId, record: &callbacks::Record) -> Option<String> {
    let interim = record
        .value
        .get("last_assistant_message")
        .and_then(serde_json::Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .map(str::to_owned);
    let tasks = record
        .value
        .get("background_tasks")
        .and_then(serde_json::Value::as_array);
    let crons = record
        .value
        .get("session_crons")
        .and_then(serde_json::Value::as_array);
    tracing::info!(
        event = "bus.callback.background_pending",
        agent_id = id.0,
        captured_reply_bytes = interim.as_ref().map_or(0, String::len),
        session_cron_count = crons.map_or(0, Vec::len),
        non_shell_task_count = tasks.map_or(0, |tasks| tasks
            .iter()
            .filter(|task| task.get("type").and_then(serde_json::Value::as_str) != Some("shell"))
            .count()),
        "Background Stop reply capture observed"
    );
    interim
}
