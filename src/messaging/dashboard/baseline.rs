//! One session field: dashboard metrics count only events after `since_ms`.
//!
//! This lives in the session data root, not the shared preference file.
//! The dashboard reads that session's state, and shared settings are a
//! different store (see the registry-session dashboard test).
//!
//! Reset writes `dashboard-stats.json` and nothing else. Messages, replies
//! and history stay where they are. A later process reads the same file,
//! so the baseline survives a restart.

use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct Stored {
    since_ms: Option<u64>,
}

pub(super) fn stats_path(root: &Path) -> PathBuf {
    root.join("dashboard-stats.json")
}

/// Events at or before the baseline are outside the stats window.
pub(super) fn after_baseline(at_ms: u64, since_ms: Option<u64>) -> bool {
    since_ms.is_none_or(|since| at_ms > since)
}

pub(super) fn since_ms(root: &Path) -> Option<u64> {
    read_since(&stats_path(root))
}

pub(super) fn label(ms: u64) -> String {
    let formatted = i64::try_from(ms / 1000)
        .ok()
        .and_then(crate::platform::local_datetime_at)
        .map(|local| {
            let clock = local.time();
            format!(
                "{} {:02}:{:02}:{:02}",
                local.date(),
                clock.hour(),
                clock.minute(),
                clock.second()
            )
        })
        .unwrap_or_else(|| "unknown time".to_owned());
    format!("since {formatted}")
}

/// Move the baseline to `now_ms`, or one millisecond later when the clock
/// has not moved. The second reset always advances.
pub(super) fn reset(root: &Path, now_ms: u64) -> io::Result<u64> {
    crate::messaging::storage::io::private_dir(root)?;
    let path = stats_path(root);
    let _lease = crate::messaging::storage::io::append_lock(&path.with_extension("json.lock"))?;
    let since_ms = match read_since(&path) {
        Some(prev) if now_ms <= prev => prev.saturating_add(1),
        _ => now_ms,
    };
    let bytes = serde_json::to_vec_pretty(&Stored {
        since_ms: Some(since_ms),
    })
    .map_err(|error| io::Error::other(error.to_string()))?;
    crate::messaging::storage::io::atomic_write(&path, &bytes)?;
    Ok(since_ms)
}

fn read_since(path: &Path) -> Option<u64> {
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice::<Stored>(&bytes) {
        Ok(stored) => stored.since_ms,
        Err(error) => {
            tracing::warn!(event = "bus.dashboard.baseline_unreadable", %error);
            None
        }
    }
}
