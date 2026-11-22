//! Native writes are independent per recipient; all model mutations stay on the coordinator.
use super::delivery::submission::{native_response_timestamps, record_native_timestamps};
use super::{diagnostics, BusState, RequestId, RoomAgent, Worker};
use crate::messaging::native::{NativeCall, NativeResult, TransportError};
use crate::protocol::api::schema::Method;
use std::{sync::mpsc, time::Instant};

pub(super) struct PendingSubmission {
    agent: RoomAgent,
    request: RequestId,
    steered: bool,
    started: Instant,
    response: mpsc::Receiver<NativeResult>,
}

impl Worker {
    pub(super) fn stamp_steering_origin(
        &self,
        state: &mut BusState,
        _agent: &RoomAgent,
        request: RequestId,
        terminal: &str,
        pane: &str,
    ) -> Result<(), String> {
        state
            .stamp_delivery_epoch(
                request,
                crate::messaging::model::DeliveryEpoch {
                    generation: self.serving_generation,
                    terminal_id: terminal.to_owned(),
                    pane_id: pane.to_owned(),
                },
            )
            .map_err(|error| error.to_string())
    }

    pub(super) fn start_native_submission(
        &mut self,
        agent: &RoomAgent,
        request: RequestId,
        method: Method,
        steered: bool,
        started: Instant,
    ) -> Result<(), String> {
        match self.transport.submit(method) {
            NativeCall::Complete(response) => {
                self.apply_native_submission(agent, request, steered, started, *response)
            }
            NativeCall::Pending(response) => {
                self.native_submissions.insert(
                    agent.id,
                    PendingSubmission {
                        agent: agent.clone(),
                        request,
                        steered,
                        started,
                        response,
                    },
                );
                Ok(())
            }
        }
    }

    /// Shutdown drains writes before releasing the coordinator lease. A restart
    /// sees every uncertain submission durably owned and must never replay it.
    pub(super) fn finish_native_submissions(&mut self, wait: bool) {
        self.finish_native_submissions_matching(wait, |_, _| true);
    }

    /// An interrupt must see the receipt before it closes a provisional
    /// group member. Claude may have refused or withdrawn the input.
    pub(in crate::messaging::coordinator) fn finish_claude_submissions_before_interrupt(
        &mut self,
        delivery: crate::messaging::model::SendMode,
        recipients: &super::AgentRecipients,
    ) {
        if delivery == crate::messaging::model::SendMode::Interrupt {
            self.finish_native_submissions_matching(true, |_, pending| {
                pending.agent.provider.uses_claude_harness()
                    && recipients.contains(&pending.agent.id)
            });
        }
    }

    /// A native write already admitted before deletion cannot be recalled.
    /// Drain its receipt before removing either endpoint from the roster.
    pub(super) fn finish_deleting_submissions(&mut self) {
        self.finish_native_submissions_matching(true, |state, pending| {
            state
                .agent(pending.agent.id)
                .is_none_or(|agent| agent.deletion_pending)
                || std::iter::once(pending.request)
                    .chain(state.group_members(pending.request))
                    .filter_map(|id| state.request(id))
                    .any(|request| state.request_source_deleting(request))
        });
    }

    fn finish_native_submissions_matching(
        &mut self,
        wait: bool,
        matches: impl Fn(&BusState, &PendingSubmission) -> bool,
    ) {
        let ids: Vec<_> = self
            .native_submissions
            .iter()
            .filter(|(_, pending)| matches(&self.state, pending))
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            let Some(pending) = self.native_submissions.get(&id) else {
                continue;
            };
            let result = if wait {
                pending
                    .response
                    .recv()
                    .map_err(|_| mpsc::TryRecvError::Disconnected)
            } else {
                pending.response.try_recv()
            };
            let response = match result {
                Err(mpsc::TryRecvError::Empty) => continue,
                Err(mpsc::TryRecvError::Disconnected) => Err(TransportError {
                    code: None,
                    message: "Native submit worker disconnected; outcome unknown".into(),
                    definitely_rejected: false,
                    timestamps: Default::default(),
                }),
                Ok(response) => response,
            };
            let Some(pending) = self.native_submissions.remove(&id) else {
                continue;
            };
            if let Err(error) = self.apply_native_submission(
                &pending.agent,
                pending.request,
                pending.steered,
                pending.started,
                response,
            ) {
                self.error = Some(error);
            }
            if self
                .state
                .request(pending.request)
                .is_some_and(|r| r.phase == super::RequestPhase::Queued)
            {
                self.submission_rechecks.insert(id);
            }
        }
    }

    fn apply_native_submission(
        &mut self,
        agent: &RoomAgent,
        request: RequestId,
        steered: bool,
        started: Instant,
        response: NativeResult,
    ) -> Result<(), String> {
        // Deletion, replacement, interrupt or recovery can settle an in-flight
        // request. A late receipt must not revive it or mutate the new owner.
        if !self
            .state
            .agent(agent.id)
            .is_some_and(|current| current.runtime_identity == agent.runtime_identity)
            || !self
                .state
                .request(request)
                .is_some_and(|r| r.phase == super::RequestPhase::Submitting)
        {
            return Ok(());
        }
        if response.is_err() && self.check_launch_directory(agent.id)?.is_some() {
            return Ok(());
        }
        let timestamps = native_response_timestamps(&response);
        let native_code = response
            .as_ref()
            .err()
            .and_then(|error| error.code.as_deref());
        if native_code == Some(crate::protocol::api::QUEUED_IN_TARGET_INPUT) {
            let mut state = self.state.clone();
            state
                .close_withdrawn_claude_input(request, steered, super::io::now_ms())
                .map_err(|error| error.to_string())?;
            record_native_timestamps(&mut state, request, timestamps)?;
            self.save(state)?;
            diagnostics::request(
                &self.state,
                request,
                "bus.delivery.result",
                crate::protocol::api::QUEUED_IN_TARGET_INPUT,
            );
            return Ok(());
        }
        let claude_turn_busy = native_code == Some(crate::protocol::api::AGENT_CLAUDE_TURN_BUSY);
        let outcome = match response {
            Err(_) if claude_turn_busy => super::SubmissionOutcome::ComposerBusy,
            Err(error)
                if steered
                    && !error.definitely_rejected
                    && error.code.as_deref() != Some(crate::protocol::api::AGENT_COMPOSER_BUSY) =>
            {
                super::SubmissionOutcome::Uncertain {
                    message: error.message,
                }
            }
            response => Self::submission_outcome(response, &agent.runtime_identity.session_id),
        };
        tracing::info!(event = "bus.delivery.result", request_id = request.0, agent_id = agent.id.0,
            steered, outcome = ?outcome, elapsed_ms = started.elapsed().as_millis() as u64,
            "Native submit completed");
        let mut state = self.state.clone();
        record_native_timestamps(&mut state, request, timestamps)?;
        if steered {
            state.record_steering(request, outcome.clone())
        } else {
            state.record_submission(request, outcome.clone())
        }
        .map_err(|error| error.to_string())?;
        if claude_turn_busy {
            state.defer_claude_delivery_until_idle(agent.id);
        }
        self.save(state)?;
        self.note_withdrawn(agent, request, &outcome);
        Ok(())
    }
}
