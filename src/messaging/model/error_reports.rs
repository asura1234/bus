//! Provider and error-report failures retain evidence without replacing unrelated diagnostics.
use super::{BusState, ModelError, Request, RequestId, SubmissionOutcome};

impl BusState {
    pub(crate) fn record_bound_provider_error(
        &mut self,
        agent: super::AgentId,
        message: &str,
        now_ms: u64,
    ) -> Result<bool, ModelError> {
        let Some(request) = self
            .agents
            .get(&agent)
            .and_then(|agent| agent.current_request)
            .filter(|request| {
                self.request(*request)
                    .is_some_and(|request| request.trusted_start_bound)
            })
        else {
            return Ok(false);
        };
        let (reason, error) = super::settlement::provider_error_outcome(message);
        let closed = self.close_turn_with_reason(request, now_ms, reason)?;
        if closed {
            if let Some(error) = self.request_failure_error(request, error) {
                if let Some(agent) = self.agents.get_mut(&agent) {
                    agent.actionable_error = Some(error);
                }
            }
        }
        Ok(closed)
    }

    /// Keep report-origin failure evidence in logs without creating a later error projection.
    pub(super) fn request_failure_error(
        &self,
        request: RequestId,
        error: String,
    ) -> Option<String> {
        if self.request(request).is_some_and(Request::is_error_report) {
            let _report = crate::utils::error_pipe::suppress(true);
            tracing::warn!(event = "bus.error_pipe.delivery_failed", request_id = request.0, %error, "Error report failed");
            None
        } else {
            Some(error)
        }
    }

    pub(super) fn consume_failed_error_report(
        &mut self,
        request: RequestId,
        outcome: &SubmissionOutcome,
    ) -> Result<bool, ModelError> {
        if !self.request(request).is_some_and(Request::is_error_report)
            || matches!(outcome, SubmissionOutcome::Confirmed { .. })
        {
            return Ok(false);
        }
        let error = match outcome {
            SubmissionOutcome::DefinitelyRejected { message }
            | SubmissionOutcome::Uncertain { message } => message.clone(),
            SubmissionOutcome::ComposerBusy => "Report composer was busy".into(),
            SubmissionOutcome::Confirmed { .. } => unreachable!(),
        };
        self.request_failure_error(request, error);
        self.close_turn_with_reason(
            request,
            crate::utils::time::now_ms(),
            "error_report_delivery_failed".into(),
        )?;
        Ok(true)
    }
}
