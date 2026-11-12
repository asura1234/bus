//! Durable per-request Feishu outbox and returned message mapping.
mod types;
use super::{AgentId, BusState};
use types::OutboxRecord;
pub(crate) use types::{AlertSource, FeishuState};

impl BusState {
    pub(crate) fn prepare_feishu_alerts(
        &mut self,
        previous: &Self,
        session: &str,
        recipient: &str,
        now_ms: u64,
    ) {
        for alert in super::developer_alerts(previous, self, true) {
            if self
                .feishu
                .outbox
                .values()
                .any(|r| r.source == alert.source)
            {
                continue;
            }
            let digest =
                crate::utils::time::digest(format!("{session}:{:?}", alert.source).as_bytes());
            let code = (12..=digest.len())
                .map(|length| digest[..length].to_owned())
                .find(|code| !self.feishu.outbox.contains_key(code));
            let Some(code) = code else { continue };
            let orchestrator = alert.sender.and_then(|id| self.feishu_target(id));
            self.feishu.outbox.insert(
                code.clone(),
                OutboxRecord {
                    text: format!("{}\n\nRequest code: bus:{code}.", alert.line),
                    key: format!("bus-{}", &digest[..40]),
                    code,
                    session: session.to_owned(),
                    source: alert.source,
                    room: alert.room,
                    raising_agent: alert.sender,
                    orchestrator,
                    recipient: recipient.to_owned(),
                    created_at_ms: now_ms,
                    attempted: false,
                    message_id: None,
                    chat_id: None,
                },
            );
        }
    }

    fn feishu_target(&self, sender: AgentId) -> Option<AgentId> {
        let agent = self.agent(sender)?;
        if agent.orchestrates.is_some() {
            Some(sender)
        } else {
            self.orchestrator_of(agent.room_id).map(|a| a.id)
        }
    }

    pub(crate) fn complete_feishu_send(
        &mut self,
        code: &str,
        message: &str,
        chat: &str,
    ) -> Result<(), String> {
        if self
            .feishu
            .outbox
            .values()
            .any(|r| r.code != code && r.message_id.as_deref() == Some(message))
        {
            return Err("Feishu message ID already maps to another request".into());
        }
        let record = self
            .feishu
            .outbox
            .get_mut(code)
            .ok_or("Unknown Feishu outbox record")?;
        if record.message_id.as_deref().is_some_and(|id| id != message)
            || record.chat_id.as_deref().is_some_and(|id| id != chat)
        {
            return Err("Feishu outbox result conflicts with the saved mapping".into());
        }
        record.message_id = Some(message.to_owned());
        record.chat_id = Some(chat.to_owned());
        record.attempted = true;
        Ok(())
    }
}
