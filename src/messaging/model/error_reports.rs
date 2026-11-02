//! Error-report failures retain evidence without replacing unrelated agent diagnostics.
use super::{BusState, ModelError, Request, RequestId, SubmissionOutcome};

impl BusState {
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
