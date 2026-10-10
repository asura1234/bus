//! Shared test-only temp paths; no component dependency.
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// A not-yet-created path under the system temp dir that no other test has been handed.
/// The clock alone is not unique: macOS reports wall time in whole microseconds, so nextest's
/// parallel test processes read the same `now_ns()` and shared one coordinator data dir. The pid
/// separates processes, the counter separates threads in one process, and the clock keeps a
/// reused pid from inheriting a dead run's leftovers. Hex keeps socket paths under `sun_path`.
pub(crate) fn unique_temp_path(prefix: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "{prefix}-{:x}-{:x}-{:x}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
        crate::utils::time::now_ns()
    ))
}
