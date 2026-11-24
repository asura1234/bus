//! Wait for the callback lock within the pane's existing transfer budget.
use super::{copy::RESTORE_BUDGET, ObservedSubmission, PromptNotShown};
use crate::platform::draft_editor::{Exchange, MarkedDraft};
use std::{
    io,
    path::Path,
    time::{Duration, Instant},
};

impl ObservedSubmission {
    pub(super) fn begin_editor_swap(&self, directory: &Path) -> io::Result<Exchange> {
        self.await_exchange(self.hard_deadline, |remaining| {
            Exchange::begin_preserving(directory.to_owned(), remaining)
        })
    }

    pub(super) fn begin_editor_restore(&self, directory: &Path) -> io::Result<Exchange> {
        self.await_exchange(self.clock.now() + RESTORE_BUDGET, |_| {
            Exchange::begin_restoration(
                directory.to_owned(),
                self.hard_deadline
                    .saturating_duration_since(self.clock.now()),
            )
        })
        .map_err(|_| io::Error::other(PromptNotShown::ComposerBusy))
    }

    pub(super) fn open_marked_draft(
        &self,
        dir: &Path,
        deadline: Instant,
    ) -> io::Result<MarkedDraft> {
        self.await_exchange(deadline, |_| MarkedDraft::open(dir))
    }

    fn await_exchange<T>(
        &self,
        deadline: Instant,
        mut begin: impl FnMut(Duration) -> io::Result<T>,
    ) -> io::Result<T> {
        let deadline = deadline.min(self.hard_deadline);
        loop {
            let generation = self.clock.generation();
            let remaining = deadline.saturating_duration_since(self.clock.now());
            if remaining.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Draft exchange lock did not become available",
                ));
            }
            match begin(remaining) {
                // Exchange also returns WouldBlock with an explicit reason for
                // semantic ownership conflicts. Only the OS lock is polled.
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock && error.get_ref().is_none() =>
                {
                    #[cfg(test)]
                    self.clock.submission_checked(generation);
                    self.clock
                        .sleep(Duration::from_millis(20).min(remaining), generation);
                }
                result => return result,
            }
        }
    }
}
