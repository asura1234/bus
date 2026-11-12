//! Launch rates over durable attempts; reading never settles pending records.
use super::metrics::{HourKey, Metric, PROVIDERS};
use crate::messaging::model::{BusState, LaunchAttempt, LaunchKind, LaunchOutcome};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Default, Serialize)]
struct Counts {
    attempts: u64,
    usable: u64,
    failed: u64,
    pending: u64,
    cancelled: u64,
    failure_reasons: BTreeMap<String, u64>,
}
impl Counts {
    fn add(&mut self, attempt: &LaunchAttempt) {
        self.attempts += 1;
        match measured_outcome(attempt) {
            LaunchOutcome::Usable => self.usable += 1,
            LaunchOutcome::Failed => {
                self.failed += 1;
                *self
                    .failure_reasons
                    .entry(cause(attempt).into())
                    .or_default() += 1;
            }
            LaunchOutcome::Pending => self.pending += 1,
            LaunchOutcome::Cancelled => self.cancelled += 1,
        }
    }
}
/// Older builds finalized dialogs at a wall-clock deadline. That proves a
/// wait, not failure or readiness; retain the raw ledger as historical evidence.
fn measured_outcome(attempt: &LaunchAttempt) -> LaunchOutcome {
    if attempt.outcome == LaunchOutcome::Failed && cause(attempt) == "dialog_timeout" {
        LaunchOutcome::Pending
    } else {
        attempt.outcome
    }
}
fn cause(attempt: &LaunchAttempt) -> &str {
    attempt
        .cause
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("missing_reason")
}
#[derive(Default, Serialize)]
struct Rates {
    launch_success: Metric,
}
impl From<&Counts> for Rates {
    fn from(c: &Counts) -> Self {
        Self {
            launch_success: Metric::new(c.usable, c.usable + c.failed),
        }
    }
}
#[derive(Default, Serialize)]
struct Series {
    counts: Counts,
    rates: Rates,
    hours: Vec<Hour>,
}
#[derive(Serialize)]
struct Hour {
    hour: String,
    start_ms: i64,
    counts: Counts,
    rates: Rates,
}
#[derive(Default)]
struct Builder {
    counts: Counts,
    hours: BTreeMap<i64, (String, Counts)>,
}
impl Builder {
    fn add(&mut self, attempt: &LaunchAttempt, hour: &HourKey) {
        self.counts.add(attempt);
        self.hours
            .entry(hour.start_ms)
            .or_insert_with(|| (hour.label.clone(), Counts::default()))
            .1
            .add(attempt);
    }
    fn finish(self) -> Series {
        let rates = Rates::from(&self.counts);
        Series {
            counts: self.counts,
            rates,
            hours: self
                .hours
                .into_iter()
                .map(|(start_ms, (hour, counts))| {
                    let rates = Rates::from(&counts);
                    Hour {
                        hour,
                        start_ms,
                        counts,
                        rates,
                    }
                })
                .collect(),
        }
    }
}
#[derive(Serialize)]
pub(super) struct Dashboard {
    tracking_started_at_ms: Option<u64>,
    all: Series,
    providers: BTreeMap<String, Series>,
    failures: Vec<Failure>,
}
#[derive(Serialize)]
struct Failure {
    id: u64,
    name: String,
    provider: &'static str,
    agent_id: Option<u64>,
    kind: LaunchKind,
    started_at_ms: u64,
    hour_start_ms: i64,
    cause: String,
    log_reference: String,
    record_reference: String,
    inspect_command: &'static str,
}
pub(super) fn snapshot(state: &BusState, clock: impl Fn(u64) -> HourKey) -> Dashboard {
    let mut all = Builder::default();
    let mut providers: BTreeMap<String, Builder> = PROVIDERS
        .into_iter()
        .map(|p| (p.into(), Builder::default()))
        .collect();
    let mut failures = Vec::new();
    for attempt in state.launch_attempts() {
        let hour = clock(attempt.started_at_ms);
        all.add(attempt, &hour);
        providers
            .entry(attempt.provider.label().into())
            .or_default()
            .add(attempt, &hour);
        if measured_outcome(attempt) == LaunchOutcome::Failed {
            failures.push(Failure {
                id: attempt.id,
                name: attempt.name.clone(),
                provider: attempt.provider.label(),
                agent_id: attempt.agent_id.map(|id| id.0),
                kind: attempt.kind,
                started_at_ms: attempt.started_at_ms,
                hour_start_ms: hour.start_ms,
                cause: cause(attempt).into(),
                log_reference: format!(
                    "herdr-client.log: bus.agent.launch_outcome attempt_id={}",
                    attempt.id
                ),
                record_reference: format!("state.json#/state/launch_attempts/{}", attempt.id),
                inspect_command: "bus --paths",
            });
        }
    }
    failures.sort_by_key(|f| std::cmp::Reverse((f.started_at_ms, f.id)));
    Dashboard {
        tracking_started_at_ms: state.launch_tracking_started_at_ms(),
        all: all.finish(),
        providers: providers
            .into_iter()
            .map(|(p, s)| (p, s.finish()))
            .collect(),
        failures,
    }
}
