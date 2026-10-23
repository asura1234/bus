use super::{
    Author, BusState, DigestSource, FactTombstone, RequestPhase, WorkerEventKind, FACT_TEXT_LIMIT,
};

impl BusState {
    /// Replay the same sources as runtime. An old source cannot become a new
    /// fact merely because its payload/tombstone is no longer retained.
    pub(crate) fn recover_room_digest_facts(&mut self) {
        let recovered_since = self.next_id;
        self.migrate_digest_facts();
        self.recover_digest_delivery();
        self.recover_digest_requests();
        self.recover_digest_events();
        self.recover_digest_notices();
        let agents: Vec<_> = self.agents().map(|a| (a.id, a.observed_at_ms)).collect();
        for (agent, at) in agents {
            self.record_digest_idle(agent, at);
        }
        for record in self.room_digests.values_mut() {
            let covered = record.watermark.max(record.pruned_through);
            record
                .facts
                .retain(|_, f| f.id < recovered_since || f.source_id > covered);
        }
        self.ack_recovered_digest_actions(recovered_since);
    }

    fn migrate_digest_facts(&mut self) {
        for record in self.room_digests.values_mut() {
            for (mut key, mut fact) in std::mem::take(&mut record.facts) {
                if key.starts_with("notice:") && key.split(':').count() == 2 {
                    if let Some(agent) = fact.agent {
                        key = format!("{key}:{}", agent.0);
                    }
                }
                let retired = fact.acknowledged_at_ms.or_else(|| {
                    (key.starts_with("notice:") && fact.agent.is_none()).then_some(fact.at_ms)
                });
                if let Some(at) = retired {
                    record.tombstones.entry(key).or_insert(FactTombstone {
                        id: fact.id,
                        agent: fact.agent,
                        acknowledged_at_ms: at,
                    });
                } else if !record.tombstones.contains_key(&key) {
                    fact.text = fact.text.chars().take(FACT_TEXT_LIMIT).collect();
                    record.facts.entry(key).or_insert(fact);
                }
            }
        }
    }

    fn recover_digest_requests(&mut self) {
        let mut replies: Vec<_> = self
            .requests()
            .filter(|r| r.phase == RequestPhase::Completed)
            .filter_map(|r| {
                r.pending_final
                    .as_ref()
                    .map(|f| (r.agent_id, r.room_id, r.group.unwrap_or(r.id), f.clone()))
            })
            .collect();
        replies.sort_by_key(|(_, _, _, reply)| reply.received_at_ms);
        for (agent, room, group, reply) in replies {
            self.record_digest_final(
                agent,
                room,
                DigestSource::new(format!("final:{}", group.0), group.0),
                &reply.text,
                reply.received_at_ms,
            );
        }
        let abandoned: Vec<_> = self
            .requests()
            .filter(|r| r.phase == RequestPhase::Abandoned)
            .map(|r| (r.id, r.completed_at_ms.unwrap_or(r.prompt.submitted_at_ms)))
            .collect();
        for (id, at) in abandoned {
            self.record_digest_request(id, at);
        }
    }

    fn recover_digest_events(&mut self) {
        let events: Vec<_> = self
            .worker_events
            .iter()
            .map(|(key, event)| (key.clone(), event.clone()))
            .collect();
        for (key, event) in events {
            self.record_digest_event(&key, &event);
            if event.kind == WorkerEventKind::Final && !key.starts_with("turn-final:") {
                if let Some((_, body)) = event.text.split_once('\n') {
                    self.record_digest_final(
                        event.agent,
                        event.room,
                        DigestSource::new(format!("event:{key}"), event.message.0),
                        body,
                        event.at_ms,
                    );
                }
            }
        }
    }

    fn recover_digest_notices(&mut self) {
        let notices: Vec<_> = self
            .requests()
            .filter(|r| r.prompt.author == Author::Bus && !r.prompt.digest_notice_agents.is_empty())
            .map(|r| (r.room_id, r.prompt.clone()))
            .collect();
        for (room, prompt) in notices {
            for about in &prompt.digest_notice_agents {
                self.record_digest_notice(
                    room,
                    Some(*about),
                    prompt.id,
                    &prompt.text,
                    prompt.submitted_at_ms,
                );
            }
        }
        let attention: Vec<_> = self
            .rooms()
            .flat_map(|r| self.notices(r.id))
            .filter_map(|p| p.attention_origin.as_ref().map(|o| (o.clone(), p.clone())))
            .collect();
        for (origin, prompt) in attention {
            if self
                .agent(origin.agent_id)
                .is_some_and(|a| a.orchestrates.is_none())
            {
                self.record_digest_notice(
                    origin.room_id,
                    Some(origin.agent_id),
                    prompt.id,
                    &prompt.text,
                    prompt.submitted_at_ms,
                );
            }
        }
    }

    fn ack_recovered_digest_actions(&mut self, recovered_since: u64) {
        let actions: Vec<_> = self
            .requests()
            .filter_map(|r| match r.prompt.author {
                Author::Agent(author)
                    if !r.prompt.broadcast && r.prompt.recipient_ids.len() == 1 =>
                {
                    Some((
                        author,
                        r.room_id,
                        r.agent_id,
                        r.prompt.submitted_at_ms,
                        r.prompt.text.clone(),
                    ))
                }
                _ => None,
            })
            .collect();
        for (orchestrator, record) in &mut self.room_digests {
            let handled: Vec<_> = record
                .facts
                .iter()
                .filter(|(_, f)| f.id >= recovered_since)
                .filter_map(|(key, fact)| {
                    actions
                        .iter()
                        .filter(|(author, room, agent, at, text)| {
                            author == orchestrator
                                && *room == fact.room
                                && *at > fact.at_ms
                                && (Some(*agent) == fact.agent
                                    || super::actions::references_fact(text, fact))
                        })
                        .map(|(_, _, _, at, _)| *at)
                        .min()
                        .map(|at| (key.clone(), at))
                })
                .collect();
            for (key, at) in handled {
                record.acknowledge(&key, at);
            }
            record.refresh_armed();
        }
    }
}
