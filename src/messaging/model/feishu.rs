//! Durable per-request Feishu outbox, inbound receipts and developer provenance.
mod routing;
mod types;
use super::{AgentId, Author, BusState, Draft};
use types::{AlertRoute, InboxRecord, OutboxRecord};
pub(crate) use types::{AlertSource, FeishuState, InReplyTo, IncomingReply, IngestOutcome};

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
            // The newest confirmed DM owns a fixed window; aliases never extend it.
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
                .max_by_key(|record| (record.created_at_ms, record.source.prompt_id))
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
                    text: format!(
                        "{}\n\nReply to this message or include bus:{code}.",
                        alert.line
                    ),
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
        self.resolve_parked_feishu()?;
        Ok(())
    }

    // A reply may beat an outbound mapping or its backfilled inbound parent.
    // Revisit exact references only; a deleted known target stays parked.
    fn resolve_parked_feishu(&mut self) -> Result<(), String> {
        let mut pending: Vec<_> = self
            .feishu
            .inbox
            .iter()
            .filter(|(_, record)| record.code.is_none())
            .map(|(key, record)| (record.received_at_ms, key.clone(), record.input.clone()))
            .collect();
        pending.sort_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)));
        for _ in 0..pending.len() {
            let mut resolved = false;
            for (_, key, input) in &pending {
                if self
                    .feishu
                    .inbox
                    .get(key)
                    .is_none_or(|record| record.code.is_some())
                {
                    continue;
                }
                let Ok(record) = self.feishu.resolve(input).cloned() else {
                    continue;
                };
                if record.recipient != input.sender_id
                    || record
                        .chat_id
                        .as_deref()
                        .is_some_and(|chat| chat != input.chat_id)
                {
                    continue;
                }
                let outcome = self.enqueue_feishu_reply(
                    &record,
                    input,
                    crate::messaging::storage::io::now_ms(),
                )?;
                if let Some(saved) = self.feishu.inbox.get_mut(key) {
                    saved.code = Some(record.code);
                    saved.outcome = outcome;
                }
                resolved = true;
            }
            if !resolved {
                break;
            }
        }
        Ok(())
    }

    pub(crate) fn ingest_feishu_reply(
        &mut self,
        input: IncomingReply,
        recipient: &str,
        now_ms: u64,
    ) -> Result<IngestOutcome, String> {
        if input.sender_id != recipient {
            return Err("Feishu sender does not match the configured recipient".into());
        }
        if input.text.trim().is_empty() || input.message_id.is_empty() || input.app_id.is_empty() {
            return Err("Feishu reply requires an app, message ID and nonempty text".into());
        }
        let key = input.key();
        if let Some(record) = self.feishu.inbox.get(&key) {
            if record.input.sender_id != input.sender_id || record.input.chat_id != input.chat_id {
                return Err("Feishu replay identity does not match the accepted message".into());
            }
            return Ok(record.outcome.clone());
        }
        let (code, outcome) = match self.feishu.resolve(&input) {
            Ok(record) => {
                if record.recipient != input.sender_id
                    || record
                        .chat_id
                        .as_deref()
                        .is_some_and(|chat| chat != input.chat_id)
                {
                    (
                        Some(record.code.clone()),
                        IngestOutcome::Parked {
                            reason:
                                "Feishu sender or chat does not match the original notification"
                                    .into(),
                        },
                    )
                } else {
                    let record = record.clone();
                    let outcome = self.enqueue_feishu_reply(&record, &input, now_ms)?;
                    (Some(record.code), outcome)
                }
            }
            Err(reason) => (None, IngestOutcome::Parked { reason }),
        };
        if let IngestOutcome::Parked { reason } = &outcome {
            let _log_only = crate::utils::error_pipe::suppress(true);
            tracing::warn!(event = "bus.feishu.parked", app_id = %input.app_id, message_id = %input.message_id, %reason, "Feishu reply remains undelivered");
        }
        let ancestry_added = code.is_some();
        self.feishu.inbox.insert(
            key,
            InboxRecord {
                input,
                code,
                outcome: outcome.clone(),
                received_at_ms: now_ms,
            },
        );
        if ancestry_added {
            self.resolve_parked_feishu()?;
        }
        Ok(outcome)
    }

    fn enqueue_feishu_reply(
        &mut self,
        record: &OutboxRecord,
        input: &IncomingReply,
        now_ms: u64,
    ) -> Result<IngestOutcome, String> {
        let Some(target) = record.orchestrator.filter(|target| {
            self.agent(*target)
                .is_some_and(|agent| !agent.deletion_pending && agent.orchestrates.is_some())
                && self
                    .room(record.room)
                    .is_some_and(|room| !room.deletion_pending)
        }) else {
            return Ok(IngestOutcome::Parked {
                reason: "Recorded room or orchestrator is unavailable".into(),
            });
        };
        let ids = self
            .submit_bus_message_with(
                record.room,
                Draft {
                    text: input.text.clone(),
                    files: Vec::new(),
                    recipient_ids: [target].into(),
                },
                Author::Developer,
                now_ms,
                true,
            )
            .map_err(|e| e.to_string())?;
        let request_id = *ids.first().ok_or("Feishu reply did not create a request")?;
        let provenance = Box::new(InReplyTo {
            session: record.session.clone(),
            source: record.source.clone(),
            room: record.room,
            raising_agent: record.raising_agent,
            orchestrator: target,
            app_id: input.app_id.clone(),
            feishu_message_id: input.message_id.clone(),
            outbound_message_id: record.message_id.clone(),
            chat_id: input.chat_id.clone(),
            sender_open_id: input.sender_id.clone(),
            create_time: input.create_time,
        });
        let request = self
            .requests
            .get_mut(&request_id)
            .ok_or("Feishu request is missing")?;
        request.prompt.in_reply_to = Some(provenance.clone());
        let message_id = request.prompt.id;
        if let Some(prompt) = self
            .rooms
            .get_mut(&record.room)
            .and_then(|room| room.latest_prompt.as_mut())
            .filter(|prompt| prompt.id == message_id)
        {
            prompt.in_reply_to = Some(provenance);
        }
        Ok(IngestOutcome::Delivered {
            message_id,
            request_id,
        })
    }
}
