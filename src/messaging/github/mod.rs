//! Room GitHub observations and durable event cursors; no provider delivery policy.
mod discovery;
mod events;
mod process;
mod runtime;
pub(crate) mod source;

pub(crate) use discovery::Interest;
pub(crate) use events::{changes, repository};
pub(crate) use runtime::{Runtime, Update};

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const POLL_MS: u64 = 90_000;
pub(crate) const LOOKBACK_MS: u64 = 86_400_000;
pub(crate) const OVERLAP_MS: u64 = 120_000;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct Watermark {
    /// Completed scan start; subsequent pages are bounded by updatedAt.
    pub(crate) merged_since_ms: u64,
    /// Fixed initial merge-time floor, independent of updated-at page movement.
    pub(crate) merged_after_ms: Option<u64>,
    pub(crate) scan_started_ms: u64,
    pub(crate) merged_cursor: Option<String>,
    pub(crate) open_cursor: Option<String>,
    pub(crate) merged_seen: BTreeMap<u64, u64>,
    pub(crate) observed: BTreeMap<u64, BTreeMap<u64, PrStatus>>,
    /// Known PR lifecycle per head; true means explicitly merged/closed.
    pub(crate) head_prs: BTreeMap<String, BTreeMap<u64, bool>>,
    pub(crate) completed_open_scans: u64,
    /// A terminal observation needs a complete open pass before retirement.
    pub(crate) retire_after_open_scan: BTreeMap<String, u64>,
}

impl Watermark {
    pub(crate) fn closed_head(&self, branch: &str) -> bool {
        self.head_prs
            .get(branch)
            .is_some_and(|prs| prs.values().all(|closed| *closed))
    }

    pub(crate) fn can_retire_head(&self, branch: &str) -> bool {
        self.closed_head(branch)
            && self
                .retire_after_open_scan
                .get(branch)
                .is_some_and(|scan| *scan <= self.completed_open_scans)
    }

    fn observe_heads(&mut self, page: &Page) {
        // A terminal observation partway through a pass must wait for the next
        // full pass; its unseen open PR may be on an already-read page.
        let retire_after = self
            .completed_open_scans
            .saturating_add(if self.open_cursor.is_none() { 1 } else { 2 });
        for (prs, closed) in [
            (&page.merged, true),
            (&page.closed, true),
            (&page.open, false),
        ] {
            for pr in prs {
                let old = self
                    .head_prs
                    .entry(pr.branch.clone())
                    .or_default()
                    .insert(pr.number, closed);
                if closed
                    && (old != Some(true) || !self.retire_after_open_scan.contains_key(&pr.branch))
                {
                    self.retire_after_open_scan
                        .insert(pr.branch.clone(), retire_after);
                }
            }
        }
        if page.open_cursor.is_none() {
            self.completed_open_scans = self.completed_open_scans.saturating_add(1);
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct PrStatus {
    pub(crate) head: String,
    pub(crate) ready: bool,
    pub(crate) ci: Option<String>,
    pub(crate) conflict: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct PullRequest {
    pub(crate) number: u64,
    pub(crate) branch: String,
    pub(crate) base: String,
    pub(crate) merged_by: String,
    pub(crate) merged_at_ms: Option<u64>,
    pub(crate) status: PrStatus,
    pub(crate) mergeable_known: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Page {
    pub(crate) default_branch: String,
    pub(crate) open: Vec<PullRequest>,
    pub(crate) merged: Vec<PullRequest>,
    pub(crate) closed: Vec<PullRequest>,
    pub(crate) open_cursor: Option<String>,
    pub(crate) merged_cursor: Option<String>,
    pub(crate) retry_at_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Scope {
    pub(crate) room: u64,
    pub(crate) branches: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Notice {
    pub(crate) room: u64,
    pub(crate) actionable: bool,
    pub(crate) text: String,
}

#[cfg(test)]
#[path = "tests/events_test.rs"]
mod tests;
