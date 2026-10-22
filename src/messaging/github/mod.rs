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
    pub(crate) merged_since_ms: u64,
    pub(crate) scan_started_ms: u64,
    pub(crate) merged_cursor: Option<String>,
    pub(crate) open_cursor: Option<String>,
    pub(crate) merged_seen: BTreeMap<u64, u64>,
    pub(crate) observed: BTreeMap<u64, BTreeMap<u64, PrStatus>>,
    pub(crate) open_seen: BTreeSet<u64>,
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
    pub(crate) author: String,
    pub(crate) merged_by: String,
    pub(crate) merged_at_ms: Option<u64>,
    pub(crate) status: PrStatus,
    pub(crate) mergeable_known: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Page {
    pub(crate) default_branch: String,
    pub(crate) branch_authors: BTreeMap<String, String>,
    pub(crate) open: Vec<PullRequest>,
    pub(crate) merged: Vec<PullRequest>,
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
