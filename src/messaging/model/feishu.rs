//! Durable per-request Feishu outbox and returned message mapping.
mod types;
use super::{AgentId, BusState};
use types::{AlertRoute, OutboxRecord};
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
                .any(|r| r.routes().any(|route| route.source == alert.source))
            {
                continue;
            }
            let orchestrator = alert.sender.and_then(|id| self.feishu_target(id));
            // The first physical DM owns a fixed window; repeats never extend it.
            // Legacy records lack ask_body and cannot safely be grouped by text.
            if let Some(record) = self
                .feishu
                .outbox
                .values_mut()
                .filter(|record| {
                    alert.sender.is_some()
                        && record.message_id.is_some()
                        && record.raising_agent == alert.sender
                        && record.session == session
                        && record.recipient == recipient
                        && record.ask_body.as_deref() == Some(alert.body.as_str())
                        && now_ms
                            .checked_sub(record.created_at_ms)
                            .is_some_and(|age| age < 30 * 60_000)
                })
                .min_by_key(|record| (record.created_at_ms, record.source.prompt_id))
            {
                record.aliases.push(AlertRoute {
                    source: alert.source,
                    room: alert.room,
                    raising_agent: alert.sender,
                    orchestrator,
                });
                continue;
            }
            let digest =
                crate::utils::time::digest(format!("{session}:{:?}", alert.source).as_bytes());
            let code = (12..=digest.len())
                .map(|length| digest[..length].to_owned())
                .find(|code| !self.feishu.outbox.contains_key(code));
            let Some(code) = code else { continue };
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
                    ask_body: Some(alert.body),
                    aliases: Vec::new(),
                    send_error: None,
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
        record.send_error = None;
        Ok(())
    }
}
