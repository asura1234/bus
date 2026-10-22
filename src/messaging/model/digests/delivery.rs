use super::{
    reply_key, AgentId, Author, BusState, Draft, ModelError, ReplyRoute, RequestId, RequestPhase,
    RuntimeStatus, FACT_TEXT_LIMIT,
};

impl BusState {
    pub(crate) fn room_digest_armed(&self, orchestrator: AgentId) -> bool {
        self.room_digests
            .get(&orchestrator)
            .is_some_and(|r| r.armed)
    }

    fn unrouted_worker_notice(&self, orchestrator: AgentId) -> bool {
        self.requests().any(|r| {
            self.reply_orchestrator(r) == Some(orchestrator)
                && self.reply_needs_ack(r)
                && matches!(
                    self.reply_routes.get(&reply_key(r)),
                    None | Some(ReplyRoute::Pending)
                )
        }) || self.worker_events.values().any(|event| {
            event.route == ReplyRoute::Pending
                && self.worker_orchestrator(event.agent) == Some(orchestrator)
        })
    }

    /// Only the ready-delivery boundary calls this. The request, pending generation
    /// and source snapshots are saved together before any native typing.
    pub(crate) fn queue_room_digest(
        &mut self,
        orchestrator: AgentId,
        now: u64,
    ) -> Result<bool, ModelError> {
        let Some(agent) = self.agent(orchestrator).filter(|a| {
            a.orchestrates.is_some()
                && a.status == RuntimeStatus::Idle
                && !a.dialog
                && !a.deletion_pending
                && a.current_request.is_none()
                && self.queued_requests(a.id).is_empty()
        }) else {
            return Ok(false);
        };
        let Some(record) = self.room_digests.get(&orchestrator) else {
            return Ok(false);
        };
        // Preserve the reply batching window: availability must not outrun
        // the actual result the owner is being asked to disposition.
        if !record.armed || self.unrouted_worker_notice(orchestrator) {
            return Ok(false);
        }
        let mut pending: Vec<_> = record
            .facts
            .values()
            .filter(|f| f.acknowledged_at_ms.is_none())
            .collect();
        let watermark = pending.iter().map(|f| f.id).max().unwrap_or(0);
        if watermark <= record.watermark {
            return Ok(false);
        }
        pending.sort_by_key(|f| (f.at_ms, f.id));
        let room = agent.orchestrates.ok_or(ModelError::InvalidTransition)?;
        let mut text = format!(
            "Room digest: {} unacknowledged facts in room {}.\n",
            pending.len(),
            room.0
        );
        for fact in pending {
            let who = fact.agent.map_or_else(
                || "bus".to_owned(),
                |id| {
                    format!(
                        "{} (agent {}){}",
                        fact.agent_name,
                        id.0,
                        if self.agent(id).is_none() {
                            " [deleted]"
                        } else {
                            ""
                        }
                    )
                },
            );
            let summary = fact.text.chars().take(FACT_TEXT_LIMIT).collect::<String>();
            text.push_str(&format!(
                "\n- Fact {}: {who}; age {}s; next owner: {} (agent {}) — {}.\n{}\n",
                fact.id,
                now.saturating_sub(fact.at_ms) / 1000,
                agent.name,
                orchestrator.0,
                fact.next_action,
                summary
            ));
        }
        text.push_str("\nAct on each fact: directly message its worker, brief another agent referencing its PR/name/head, close the worker, or explicitly dismiss it with bus digest ack FACT_ID. Reading this digest or replying to it does not acknowledge facts. No repeated wake is scheduled without new facts.");
        let request = self.submit_message_with(
            room,
            Draft {
                text,
                files: vec![],
                recipient_ids: [orchestrator].into(),
            },
            Author::Bus,
            now,
            true,
        )?[0];
        if let Some(record) = self.room_digests.get_mut(&orchestrator) {
            record.previous_watermark = record.watermark;
            record.pending_watermark = Some(watermark);
            record.armed = false;
            record.last_digest_request = Some(request);
        }
        tracing::info!(
            event = "bus.digest.queued",
            agent_id = orchestrator.0,
            room_id = room.0,
            watermark,
            request_id = request.0,
            "Room digest queued for a clean turn"
        );
        Ok(true)
    }

    pub(in crate::messaging::model) fn is_room_digest(&self, id: RequestId) -> bool {
        self.room_digests
            .values()
            .any(|r| r.last_digest_request == Some(id))
    }
}

impl BusState {
    pub(in crate::messaging::model) fn mark_digest_delivered(&mut self, request: RequestId) {
        let at = self
            .request(request)
            .map_or(0, |r| r.progress_at_ms.unwrap_or(r.prompt.submitted_at_ms));
        for record in self
            .room_digests
            .values_mut()
            .filter(|r| r.last_digest_request == Some(request))
        {
            if let Some(watermark) = record.pending_watermark.take() {
                record.watermark = watermark;
                record.last_digest_at_ms = Some(at);
                record.refresh_armed();
            }
        }
    }

    pub(super) fn restore_abandoned_digest(&mut self, request: RequestId) {
        if !self
            .request(request)
            .is_some_and(|r| r.phase == RequestPhase::Abandoned)
        {
            return;
        }
        for record in self
            .room_digests
            .values_mut()
            .filter(|r| r.last_digest_request == Some(request))
        {
            record.watermark = record.previous_watermark;
            record.pending_watermark = None;
            record.refresh_armed();
        }
    }

    pub(super) fn recover_digest_delivery(&mut self) {
        let records: Vec<_> = self
            .room_digests
            .iter()
            .filter_map(|(agent, r)| r.last_digest_request.map(|id| (*agent, id)))
            .collect();
        for (agent, id) in records {
            let Some(request) = self.request(id) else {
                continue;
            };
            let phase = request.phase;
            let delivered = request.trusted_start_bound
                || phase == RequestPhase::Completed
                || phase == RequestPhase::Active && !request.uncertain_outcome;
            // Read the pre-fix queued watermark as pending, never as delivery.
            if !delivered && matches!(phase, RequestPhase::Queued | RequestPhase::Submitting) {
                if let Some(record) = self
                    .room_digests
                    .get_mut(&agent)
                    .filter(|r| r.pending_watermark.is_none())
                {
                    record.pending_watermark = Some(record.watermark);
                    record.watermark = record.previous_watermark;
                }
            }
            if phase == RequestPhase::Abandoned {
                self.restore_abandoned_digest(id);
            } else if delivered {
                self.mark_digest_delivered(id);
            }
        }
    }
}
