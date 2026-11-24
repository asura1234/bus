//! Defer copy recovery diagnostics until the submission releases its lease.
use super::{report_disconnect, Abandoned};
use std::{cell::RefCell, io};

#[derive(Default)]
pub(in crate::terminal::runtime::submission) struct CallerDiagnostics {
    pending: RefCell<Vec<Diagnostic>>,
}

enum Diagnostic {
    AdmissionSkipped(&'static str),
    StartupFailed(String),
    OverflowOutcomeDropped,
    Disconnected(&'static str, Abandoned),
    CopyLoss(&'static str, io::Error),
}

impl CallerDiagnostics {
    pub(super) fn admission_skipped(&self, reason: &'static str) {
        self.pending
            .borrow_mut()
            .push(Diagnostic::AdmissionSkipped(reason));
    }

    pub(super) fn startup_failed(&self, error: String) {
        self.pending
            .borrow_mut()
            .push(Diagnostic::StartupFailed(error));
    }

    pub(super) fn overflow_outcome_dropped(&self) {
        self.pending
            .borrow_mut()
            .push(Diagnostic::OverflowOutcomeDropped);
    }

    pub(super) fn disconnected(&self, reason: &'static str, abandoned: Option<Abandoned>) {
        if let Some(abandoned) = abandoned {
            self.pending
                .borrow_mut()
                .push(Diagnostic::Disconnected(reason, abandoned));
        }
    }

    pub(in crate::terminal::runtime::submission) fn copy_loss(
        &self,
        reason: &'static str,
        error: io::Error,
    ) {
        self.pending
            .borrow_mut()
            .push(Diagnostic::CopyLoss(reason, error));
    }

    pub(in crate::terminal::runtime::submission) fn emit(&self) {
        for diagnostic in self.pending.take() {
            match diagnostic {
                Diagnostic::AdmissionSkipped(reason) => tracing::warn!(
                    event = "bus.draft.copy_recovery_failed",
                    reason,
                    "Copied draft persistence was not queued"
                ),
                Diagnostic::StartupFailed(error) => tracing::warn!(
                    event = "bus.draft.copy_recovery_failed",
                    reason = "snapshot_worker_unavailable",
                    %error,
                    "Could not start copied draft storage"
                ),
                Diagnostic::OverflowOutcomeDropped => tracing::warn!(
                    event = "bus.draft.copy_recovery_failed",
                    reason = "snapshot_overflow_outcome_dropped",
                    "Copied draft outcome could not be tracked beyond the overflow limit"
                ),
                Diagnostic::Disconnected(reason, abandoned) => {
                    report_disconnect(reason, Some(abandoned));
                }
                Diagnostic::CopyLoss(reason, error) => tracing::warn!(
                    event = "bus.draft.copy_loss",
                    reason,
                    %error,
                    "Copied draft restoration was not proven and no recovery record was scheduled"
                ),
            }
        }
    }
}
