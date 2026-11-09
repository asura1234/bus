//! Closed coordinator policy over generic native menu identities. Parsing,
//! focus navigation and enqueue freshness remain owned by the terminal driver.
use super::{AgentId, Provider, Worker};
use serde_json::json;

// Semantic replay provenance: tests/fixtures/provider-ui/README.md. The driver
// producer tests pin this content identity independently of wraps and key hints.
const LATENCY_MENU: &str = "33415108d1173ade22a8d5b1cf5b92e1e687caa413866bc2071184578a2b8b14";
const KEEP_WAITING: &str = "Dismiss and keep waiting";

impl Worker {
    /// Called only after notify_dialogs verified the current native blocker.
    /// True suppresses help for this poll after a completed label selection;
    /// every spent, uncertain or pending attempt returns to ordinary help.
    pub(super) fn answer_known_latency(
        &mut self,
        id: AgentId,
        current: &str,
    ) -> Result<bool, String> {
        let agent = self.state.agent(id).ok_or("Unknown agent")?;
        if current != LATENCY_MENU {
            // Another verified menu ends this episode without requiring an
            // intervening idle screen. Fallback blocked metadata proves no
            // replacement identity and must not renew an uncertain attempt.
            if current != super::dialogs::BLOCKED && agent.automatic_dialog_answer.is_some() {
                let mut state = self.state.clone();
                state
                    .clear_automatic_dialog_answer(id)
                    .map_err(|error| error.to_string())?;
                self.save(state)?;
            }
            return Ok(false);
        }
        if !matches!(
            agent.provider,
            Provider::ClaudeCode | Provider::ClaudeDs | Provider::Codex
        ) {
            return Ok(false);
        }
        let (terminal, _, _) = self.terminal_identity(id)?;
        let reservation = serde_json::to_string(&(&agent.runtime_identity, current))
            .map_err(|error| error.to_string())?;
        if agent.automatic_dialog_answer.as_deref() == Some(reservation.as_str()) {
            return Ok(false);
        }

        // Reserve durably before the first arrow, including an uncertain write.
        // A coordinator restart cannot replay an attempt whose reply was lost.
        let mut state = self.state.clone();
        state
            .reserve_automatic_dialog_answer(id, reservation)
            .map_err(|error| error.to_string())?;
        self.save(state)?;

        let selected = match self.execute_driver(
            "agent.press",
            &json!({
                "agent": id.0.to_string(),
                "terminal_id": terminal,
                "select": KEEP_WAITING,
            }),
        ) {
            Ok(result) => result["outcome"] == "selected",
            Err(error) => {
                tracing::warn!(event = "bus.automatic_dialog.failed", agent_id = id.0,
                    reason = %error, "Latency notice selection did not finish");
                false
            }
        };
        Ok(selected)
    }
}
