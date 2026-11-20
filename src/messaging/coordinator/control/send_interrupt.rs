//! Durable dispatch of stops for already queued interrupt-mode sends.
use super::{
    schema, AgentId, Author, Method, RequestId, RequestPhase, ResponseResult, RoomId, Worker,
};
use crate::messaging::model::{BusState, SendInterrupt, SendInterruptOutcome, SendMode};
use crate::messaging::storage::state_store::JournalOp;
use serde_json::{json, Value};

pub(in crate::messaging::coordinator) fn stage_send_interrupts(
    state: &mut BusState,
    requests: &[RequestId],
) -> Result<Vec<(RequestId, SendInterrupt)>, String> {
    state
        .record_send_mode(requests, SendMode::Interrupt)
        .map_err(|e| e.to_string())?;
    let mut interrupts = Vec::new();
    for id in requests {
        state
            .mark_interrupt_delivery(*id)
            .map_err(|error| error.to_string())?;
        let request = state.request(*id).ok_or("Missing queued send")?;
        if let Some(intent) = state.send_interrupt_target(request.agent_id) {
            state
                .stage_send_interrupt(*id, intent.clone())
                .map_err(|error| error.to_string())?;
            interrupts.push((*id, intent));
        }
    }
    Ok(interrupts)
}

impl Worker {
    pub(in crate::messaging::coordinator) fn replay_queued_send_interrupts(
        &mut self,
        now: u64,
    ) -> Result<(), String> {
        let pending: Vec<_> = self
            .state
            .requests()
            .filter(|request| {
                request.phase == RequestPhase::Queued && request.send_interrupt.is_some()
            })
            .map(|request| request.id)
            .collect();
        for id in pending {
            self.dispatch_queued_send_interrupt(id, now, true)?;
        }
        Ok(())
    }

    /// The send is already durable. Keep its receipt even if stop persistence pauses.
    pub(in crate::messaging::coordinator) fn dispatch_committed_send_interrupts(
        &mut self,
        ids: &[RequestId],
        now: u64,
    ) -> Vec<Value> {
        let mut paused = None;
        ids.iter().map(|id| {
            if paused.is_none() {
                match self.dispatch_queued_send_interrupt(*id, now, false) {
                    Ok(result) => return result,
                    Err(error) => {
                        tracing::warn!(%error, request_id = id.0, "Queued send stop dispatch paused");
                        paused = Some(error);
                    }
                }
            }
            let request = self.state.request(*id).expect("durably queued send");
            let outcome = request.send_interrupt_outcome.as_ref()
                .map_or("pending", |result| result.outcome.as_str());
            json!({"agent_id": request.agent_id, "outcome": outcome, "reason": paused})
        }).collect()
    }

    fn dispatch_queued_send_interrupt(
        &mut self,
        id: RequestId,
        now: u64,
        replaying: bool,
    ) -> Result<Value, String> {
        let request = self.state.request(id).ok_or("Missing queued send")?;
        let (room, recipient) = (request.room_id, request.agent_id);
        let sender = request.prompt.author.clone();
        let staged = request.send_interrupt.clone();
        let current = self.state.send_interrupt_target(recipient);
        let same_target = match (&staged, &current) {
            (Some(staged), Some(current)) => {
                staged.same_turn(current)
                    && staged.identity.pane_id == current.identity.pane_id
                    && staged.activity == current.activity
            }
            (None, None) => true,
            _ => false,
        };
        let agent = self.state.agent(recipient).ok_or("Unknown agent")?;
        let needs_stop = !agent.interrupt_pending && self.state.needs_interrupt_stop(agent);
        let mut state = self.state.clone();
        let mut release = None;
        let unverified = replaying
            && staged.as_ref().is_some_and(|intent| {
                intent.target.is_none()
                    && (!intent.replayable()
                        || current.as_ref().is_none_or(|now| !now.replayable()))
            });
        let result = if unverified {
            json!({"agent_id": recipient, "outcome": "skipped",
                "reason": "The autonomous turn cannot be revalidated"})
        } else if !agent.interrupt_pending && !same_target {
            json!({"agent_id": recipient, "outcome": "skipped",
                "reason": "The original interrupt target changed"})
        } else if needs_stop
            && current
                .as_ref()
                .is_some_and(|intent| self.state.send_interrupt_already_claimed(recipient, intent))
        {
            json!({"agent_id": recipient, "outcome": "unknown",
                "reason": "The prior stop may have reached the terminal"})
        } else if needs_stop
            && staged
                .as_ref()
                .and_then(|intent| intent.identity.terminal_id.as_deref())
                .is_none_or(str::is_empty)
        {
            json!({"agent_id": recipient, "outcome": "failed",
                "reason": "The stop target has no confirmed terminal identity"})
        } else {
            let claimed = needs_stop && staged.is_some();
            if claimed {
                // Commit only this recipient's claim before a possible native write.
                let op = JournalOp::SendInterruptProgress {
                    room,
                    request_id: id,
                    claim: true,
                    release: None,
                    outcome: SendInterruptOutcome {
                        outcome: "unknown".into(),
                        reason: None,
                    },
                };
                self.persist_send_interrupt_progress(state, op)?;
                state = self.state.clone();
            }
            let (result, may_have_written) = self.interrupt_queued_recipient(
                &mut state,
                room,
                recipient,
                &sender,
                now,
                staged.as_ref(),
            );
            if claimed && !may_have_written {
                release = staged;
            }
            if may_have_written {
                state.retain_interrupt_stop_claim(recipient);
            }
            result
        };
        // Interrupt-mode sends never wait for the previous turn to finish:
        // settle the interrupted turn immediately for every stop outcome so the
        // new message can deliver as soon as the pane accepts input. Keep an
        // in-flight/uncertain provisional busy-join head intact so its native
        // receipt can finish or requeue cleanly.
        let provisional_submitting = state
            .agent(recipient)
            .and_then(|agent| agent.current_request)
            .and_then(|request_id| state.request(request_id))
            .is_some_and(|request| {
                request.phase == RequestPhase::Submitting && !request.trusted_start_bound
            });
        if !provisional_submitting
            && state
                .agent(recipient)
                .is_some_and(|agent| agent.current_request.is_some() || agent.interrupt_pending)
        {
            state
                .complete_native_interrupt(recipient, now)
                .map_err(|error| error.to_string())?;
        }
        let outcome = SendInterruptOutcome {
            outcome: result["outcome"]
                .as_str()
                .ok_or("Missing stop outcome")?
                .into(),
            reason: result["reason"].as_str().map(str::to_owned),
        };
        self.persist_send_interrupt_progress(
            state,
            JournalOp::SendInterruptProgress {
                room,
                request_id: id,
                claim: false,
                outcome,
                release,
            },
        )?;
        Ok(result)
    }

    /// Best-effort stop for one recipient of an already durable queued send.
    fn interrupt_queued_recipient(
        &mut self,
        state: &mut BusState,
        room: RoomId,
        recipient: AgentId,
        interrupter: &Author,
        now: u64,
        staged: Option<&SendInterrupt>,
    ) -> (Value, bool) {
        if state
            .agent(recipient)
            .is_some_and(|agent| agent.interrupt_pending)
        {
            // A second key can exit the provider or open Claude's rewind.
            return (
                json!({"agent_id": recipient, "outcome": "interrupting"}),
                false,
            );
        }
        let mut may_have_written = false;
        let result = match self.interrupt_agent(
            state,
            recipient,
            room,
            interrupter,
            now,
            false,
            Some(&mut may_have_written),
            staged,
        ) {
            Ok(result) => result,
            Err(reason) => {
                tracing::warn!(
                    event = "bus.send.interrupt_failed",
                    agent_id = recipient.0,
                    room_id = room.0,
                    reason,
                    "Interrupt-mode delivery could not stop the running turn"
                );
                json!({"agent_id": recipient, "outcome": "failed", "reason": reason})
            }
        };
        (result, may_have_written)
    }

    pub(super) fn send_stop_keys(
        &mut self,
        pane: String,
        key: &str,
        terminal: Option<String>,
        staged: Option<&SendInterrupt>,
        native_interrupt: Option<schema::NativeInterruptIdentity>,
    ) -> Result<ResponseResult, crate::messaging::native::TransportError> {
        self.transport
            .request(Method::AgentSendKeys(schema::AgentSendKeysParams {
                target: pane,
                keys: vec![key.into()],
                expected_terminal_id: staged
                    .map_or(terminal, |intent| intent.identity.terminal_id.clone()),
                expected_activity: staged.and_then(|intent| intent.activity).map(
                    |(generation, state_change_seq)| schema::AgentKeyActivity {
                        generation,
                        state_change_seq,
                    },
                ),
                native_interrupt,
            }))
    }

    fn persist_send_interrupt_progress(
        &mut self,
        mut state: BusState,
        op: JournalOp,
    ) -> Result<(), String> {
        let journal_only = state == self.state;
        op.apply(&mut state).map_err(|error| error.to_string())?;
        if journal_only {
            self.journal(state, op)
        } else {
            self.save(state)
        }
    }
}
