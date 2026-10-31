//! Withdraw a Bus prompt that Ctrl+C restored into the Grok composer.
use super::Worker;
use crate::agents::grok_surface::restored_prompt_clear_keys;
use crate::messaging::model::Provider;
use crate::protocol::api::schema::{self, Method, ResponseResult};

impl Worker {
    /// Ctrl+C on Grok's "Waiting for response…" unsends Bus's prompt into the
    /// composer. That screen stays working until this owned line-kill runs, so
    /// the next delivery is not refused for an occupied composer.
    pub(super) fn withdraw_restored_grok_prompts(&mut self) {
        let panes: Vec<_> = self
            .state
            .agents()
            .filter(|agent| {
                agent.provider == Provider::Grok
                    && agent.interrupt_pending
                    && agent.runtime_identity.pane_id.is_some()
            })
            .map(|agent| {
                (
                    agent.id,
                    agent.runtime_identity.pane_id.clone().unwrap_or_default(),
                )
            })
            .collect();
        for (id, pane) in panes {
            let output = self
                .transport
                .request(Method::AgentRead(schema::AgentReadParams {
                    target: pane.clone(),
                    source: schema::ReadSource::Visible,
                    lines: None,
                    format: schema::ReadFormat::Text,
                    strip_ansi: true,
                }));
            let Ok(ResponseResult::PaneRead { read }) = output else {
                continue;
            };
            let Some(keys) = restored_prompt_clear_keys(&read.text) else {
                continue;
            };
            tracing::info!(
                event = "bus.agent.interrupt.withdraw",
                agent_id = id.0,
                keys = keys.len(),
                "Withdrawing the prompt Ctrl+C restored into the Grok composer"
            );
            if let Err(error) =
                self.transport
                    .request(Method::AgentSendKeys(schema::AgentSendKeysParams {
                        target: pane,
                        keys,
                    }))
            {
                tracing::warn!(
                    event = "bus.agent.interrupt.withdraw",
                    agent_id = id.0,
                    error = %error.message,
                    "Could not withdraw the restored Grok prompt"
                );
            }
        }
    }
}
