//! Native writes are independent per recipient; all model mutations stay on the coordinator.
use super::delivery::submission::{native_response_timestamps, record_native_timestamps};
use super::{AgentId, RequestId, RoomAgent, Worker};
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
        let ids: Vec<AgentId> = self.native_submissions.keys().copied().collect();
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
        let outcome = match response {
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
        self.save(state)?;
        self.note_withdrawn(agent, request, &outcome);
        Ok(())
    }
}
