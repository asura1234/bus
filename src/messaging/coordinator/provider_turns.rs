//! Provider turn facts read beyond the hook's own verdict: background shells
//! a Claude turn is still waiting on, and Codex usage from its rollout.
use super::{callbacks, AgentId, BTreeSet, Parsed, Path, Provider, ProviderCallback, Worker};

impl Worker {
    /// A Claude reply waits while shells started by its request still run:
    /// each wakes a turn when it ends, and the last such turn carries the real
    /// reply. Shells inherited from an earlier request (say a hung
    /// `find /`) are listed by every later `Stop` and never hold a reply.
    /// A held `Stop` also returns its interim text, which the hold cap delivers.
    pub(super) fn hold_for_started_shells(
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
            tracing::info!(
                event = "bus.callback.background_shells",
                agent_id = id.0,
                shells = ?started,
                "Reply waits for shells this request started"
            );
            return (Parsed::BackgroundPending { session, turn }, Some(text));
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
        let Some(body) = callback.prompt_payload.as_deref().and_then(|prompt| {
            prompt
                .trim()
                .strip_prefix("<task-notification>")?
                .strip_suffix("</task-notification>")
        }) else {
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
