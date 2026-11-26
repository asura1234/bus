//! Render attributed prompt batches.
use super::{Author, BusState, RequestId};
use crate::messaging::model::BUS_SENDER;

impl BusState {
    pub(in crate::messaging::model) fn render_coalesced_payload(
        &mut self,
        parts: &[RequestId],
    ) -> String {
        let count = parts.len();
        parts
            .iter()
            .enumerate()
            .filter_map(|(index, id)| {
                let payload = self.render_request_payload(*id)?;
                let prompt = &self.requests.get(id)?.prompt;
                let attribution = match prompt.author {
                    Author::Agent(_) | Author::Bot(_) => String::new(),
                    _ => format!(" from {}", self.sender_name(&prompt.author)),
                };
                Some(format!(
                    "[{}/{count}{attribution} at {}]\n{}",
                    index + 1,
                    clock(prompt.submitted_at_ms),
                    payload
                ))
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn sender_name(&self, author: &Author) -> String {
        match author {
            Author::Developer => "the developer".into(),
            Author::Bus => BUS_SENDER.into(),
            Author::Bot(name) => name.clone(),
            Author::Agent(id) => self
                .agents
                .get(id)
                .map_or_else(|| "an agent".into(), |agent| agent.name.clone()),
        }
    }
}

/// Local wall-clock `HH:MM` of a millisecond timestamp, for coalesced prompts.
fn clock(at_ms: u64) -> String {
    i64::try_from(at_ms / 1000)
        .ok()
        .and_then(crate::platform::local_datetime_at)
        .map(|local| format!("{:02}:{:02}", local.hour(), local.minute()))
        .unwrap_or_else(|| "--:--".into())
}
