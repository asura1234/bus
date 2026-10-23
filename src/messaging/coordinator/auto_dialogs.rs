//! Automatic answers are a closed list of provider-owned native UI shapes.
use super::{schema::AgentDialogChoiceAction as Action, AgentId, Provider, Worker};
use sha2::{Digest, Sha256};

pub(super) enum AutomaticObservation {
    Ready(String, u32),
    Changed,
    Unavailable,
}

struct Rule {
    provider: Provider,
    shape: &'static str,
    name: &'static str,
    option: u32,
}

// Recorded native title + kind + every numbered option, not terminal text.
// This action dismisses a latency notice while retaining the current model.
// Command, file, permission and update approvals must never enter this list.
const RULES: &[Rule] = &[
    Rule {
        provider: Provider::ClaudeCode,
        shape: "6105abde0622e6a34d10c0743d1ad884f13286da890fb7882cf0618c7fbf83a8",
        name: "claude_extra_thought",
        option: 2,
    },
    Rule {
        provider: Provider::Codex,
        shape: "6105abde0622e6a34d10c0743d1ad884f13286da890fb7882cf0618c7fbf83a8",
        name: "codex_extra_thought",
        option: 2,
    },
];

impl Worker {
    /// Returns true while automatic handling is in progress. A spent or
    /// stalled attempt falls back to the ordinary once-only help notice.
    pub(super) fn auto_answer_dialog(
        &mut self,
        id: AgentId,
        shape: Option<&str>,
    ) -> Result<bool, String> {
        let agent = self.state.agent(id).ok_or("Unknown agent")?;
        let Some(rule) = RULES
            .iter()
            .find(|rule| rule.provider == agent.provider && shape == Some(rule.shape))
        else {
            if agent.automatic_dialog_answer.is_some() {
                self.clear_automatic_dialog_answer(id)?;
            }
            return Ok(false);
        };
        let identity = serde_json::to_vec(&(&agent.runtime_identity, rule.shape))
            .map_err(|error| error.to_string())?;
        let episode = format!("{:x}", Sha256::digest(identity));
        let focused = format!("focus:{episode}");
        let answered = format!("answer:{episode}");
        let previous = agent.automatic_dialog_answer.clone();
        if previous.as_deref() == Some(answered.as_str())
            || (previous.as_deref() == Some(focused.as_str())
                && agent.dialog_notice.as_deref() == Some(rule.shape))
        {
            return Ok(false);
        }
        // Re-observe and check the *fresh* claims. A replacement permission
        // dialog must never lend its new fingerprint to the old option number.
        let (fingerprint, selected) = match self.observe_allowlisted_fingerprint(id, rule.shape) {
            Ok(AutomaticObservation::Ready(fingerprint, selected)) => (fingerprint, selected),
            // Defer a replacement to the next poll rather than reporting the
            // previous shape as blocked after we already saw it change.
            Ok(AutomaticObservation::Changed) => return Ok(true),
            _ => return Ok(false),
        };
        let (action, reservation) = if selected == rule.option {
            (Action::Confirm, answered)
        } else if previous.as_deref() == Some(focused.as_str()) {
            // Hand off a stalled focus without moving or confirming again.
            return Ok(false);
        } else {
            (Action::Focus, focused.clone())
        };
        self.reserve_automatic_dialog_answer(id, reservation)?;
        tracing::info!(event = "bus.dialog.auto_answer", agent_id = id.0,
            rule = rule.name, shape = rule.shape, option = rule.option,
            action = ?action, outcome = "attempt", "Attempting an allowlisted provider UI answer");
        match self.choose_automatic_dialog_option(id, rule.option, &fingerprint, action) {
            Ok(answer) => {
                tracing::info!(
                    event = "bus.dialog.auto_answer",
                    agent_id = id.0,
                    rule = rule.name,
                    option = rule.option,
                    outcome = answer["outcome"].as_str().unwrap_or("unknown"),
                    "Allowlisted provider UI answer returned"
                );
            }
            Err(error) if error.starts_with("No keys were sent (") => {
                // A definitive native refusal permits a fresh guarded attempt
                // after redraw. An uncertain response never permits replay.
                self.clear_automatic_dialog_answer(id)?;
                tracing::debug!(
                    event = "bus.dialog.auto_answer",
                    agent_id = id.0,
                    rule = rule.name,
                    outcome = "not_sent",
                    "Native guard refused the automatic answer"
                );
            }
            Err(_) => {
                tracing::warn!(
                    event = "bus.dialog.auto_answer",
                    agent_id = id.0,
                    rule = rule.name,
                    outcome = "uncertain",
                    "Automatic answer may have been sent; it will not be replayed"
                );
            }
        }
        Ok(true)
    }

    fn reserve_automatic_dialog_answer(
        &mut self,
        id: AgentId,
        episode: String,
    ) -> Result<(), String> {
        let mut state = self.state.clone();
        state
            .set_automatic_dialog_answer(id, Some(episode))
            .map_err(|error| error.to_string())?;
        self.save(state)
    }

    pub(super) fn clear_automatic_dialog_answer(&mut self, id: AgentId) -> Result<(), String> {
        let mut state = self.state.clone();
        state
            .set_automatic_dialog_answer(id, None)
            .map_err(|error| error.to_string())?;
        self.save(state)
    }
}
