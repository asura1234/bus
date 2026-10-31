//! Crash-copy compaction for history, kept off the serving thread.
use super::Worker;

impl Worker {
    /// Before serving anything, deduplicates crash copies in history, so the
    /// rewrite never runs on the UI thread or between commands.
    pub(super) fn compact_history(&self) {
        match self.store.compact_history() {
            Ok(true) => tracing::info!(
                event = "bus.history.compacted",
                entries = self.state.history().len(),
                "Bus history rewritten without duplicate crash copies"
            ),
            Ok(false) => {}
            Err(error) => tracing::warn!(
                event = "bus.history.compact_failed",
                %error,
                "Bus history keeps duplicate crash copies until the next start"
            ),
        }
    }
}
