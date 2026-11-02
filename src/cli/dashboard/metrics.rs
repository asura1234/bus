//! Read-only delivery metrics over the same live/archive requests as message status.
use crate::messaging::model::{BusState, Request, RequestPhase};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Default, Serialize)]
pub(super) struct Counts {
    pub(super) attempts: u64,
    pub(super) delivered: u64,
    pub(super) failed: u64,
    pub(super) open: u64,
    pub(super) replied: u64,
    pub(super) reply_not_captured: u64,
    pub(super) reply_pending: u64,
    pub(super) bus_failures: u64,
    pub(super) external: u64,
    pub(super) failure_reasons: BTreeMap<String, u64>,
    pub(super) bus_reasons: BTreeMap<String, u64>,
    pub(super) external_reasons: BTreeMap<String, u64>,
    pub(super) terminal_reasons: BTreeMap<String, u64>,
    pub(super) stages: BTreeMap<String, u64>,
}
impl Counts {
    pub(super) fn delivery_rate(&self) -> Option<f64> {
        rate(self.delivered, self.delivered + self.failed)
    }
    pub(super) fn reply_rate(&self) -> Option<f64> {
        rate(self.replied, self.replied + self.reply_not_captured)
    }
    pub(super) fn bus_rate(&self) -> Option<f64> {
        rate(self.replied, self.replied + self.bus_failures)
    }

    fn add(&mut self, request: &Request, outcome: &Outcome) {
        self.attempts += 1;
        match outcome.delivery {
            "delivered" => self.delivered += 1,
            "failed" => {
                self.failed += 1;
                increment(&mut self.failure_reasons, reason(request));
            }
            _ => self.open += 1,
        }
        match outcome.capture {
            "replied" => self.replied += 1,
            "missing" => self.reply_not_captured += 1,
            "pending" if outcome.delivery == "delivered" => self.reply_pending += 1,
            _ => {}
        }
        match outcome.cause {
            "bus" => {
                self.bus_failures += 1;
                increment(&mut self.bus_reasons, reason(request));
            }
            "external" => {
                self.external += 1;
                increment(&mut self.external_reasons, reason(request));
            }
            _ => {}
        }
        increment(&mut self.stages, stage(request));
        if request.settled() && outcome.capture != "replied" {
            increment(&mut self.terminal_reasons, reason(request));
        }
    }
}
fn rate(n: u64, d: u64) -> Option<f64> {
    (d != 0).then(|| n as f64 * 100.0 / d as f64)
}
fn increment(map: &mut BTreeMap<String, u64>, key: &str) {
    *map.entry(key.to_owned()).or_default() += 1;
}
fn reason(request: &Request) -> &str {
    request
        .failure_reason
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .unwrap_or("missing_reason")
}
fn stage(request: &Request) -> &str {
    if request.reply().is_some() {
        return "replied";
    }
    match request.phase {
        RequestPhase::Queued => "queued",
        RequestPhase::Submitting => "submitting",
        RequestPhase::Active if request.group.is_some() => "joined",
        RequestPhase::Active if request.trusted_start_bound => "delivered",
        RequestPhase::Active => "awaiting_start",
        RequestPhase::Completed => "replied",
        RequestPhase::Abandoned => "abandoned",
    }
}
#[derive(Default, Serialize)]
pub(super) struct Series {
    pub(super) counts: Counts,
    pub(super) hours: Vec<Hour>,
    rates: Rates,
}
#[derive(Default, Serialize)]
struct Rates {
    delivery: Option<f64>,
    reply_capture: Option<f64>,
    bus_reliability: Option<f64>,
}
impl From<&Counts> for Rates {
    fn from(c: &Counts) -> Self {
        Self {
            delivery: c.delivery_rate(),
            reply_capture: c.reply_rate(),
            bus_reliability: c.bus_rate(),
        }
    }
}
#[derive(Serialize)]
pub(super) struct Hour {
    pub(super) hour: String,
    pub(super) start_ms: i64,
    pub(super) counts: Counts,
    rates: Rates,
}
#[derive(Default, Serialize)]
pub(super) struct Dashboard {
    pub(super) all: Series,
    pub(super) providers: BTreeMap<String, Series>,
}
#[derive(Clone)]
pub(super) struct HourKey {
    pub(super) start_ms: i64,
    pub(super) label: String,
}
struct Outcome {
    delivery: &'static str,
    capture: &'static str,
    cause: &'static str,
}
fn pre_start_failure(request: &Request) -> bool {
    matches!(
        reason(request),
        "submission_unconfirmed"
            | "prompt_not_shown"
            | "delivery_error"
            | "prior_request_active"
            | "not_submitted"
            | "session_hook_missing"
    )
}
fn bound(state: &BusState, request: &Request) -> bool {
    request.trusted_start_bound
        || (!request.uncertain_outcome
            && matches!(
                request.phase,
                RequestPhase::Active | RequestPhase::Abandoned
            )
            && request
                .group
                .and_then(|id| state.request(id))
                .is_some_and(|lead| {
                    lead.agent_id == request.agent_id
                        && !pre_start_failure(lead)
                        && (lead.trusted_start_bound
                            || lead.reply().is_some()
                            || lead.phase == RequestPhase::Completed)
                }))
}
fn classify(state: &BusState, request: &Request) -> Outcome {
    if request.reply().is_some() || request.phase == RequestPhase::Completed {
        return Outcome {
            delivery: "delivered",
            capture: "replied",
            cause: "success",
        };
    }
    let terminal = request.phase == RequestPhase::Abandoned;
    let delivered = bound(state, request) && !(terminal && pre_start_failure(request));
    let delivery = if delivered {
        "delivered"
    } else if terminal {
        "failed"
    } else {
        "open"
    };
    let capture = if !terminal {
        "pending"
    } else if delivered {
        "missing"
    } else {
        "none"
    };
    let cause = if !terminal {
        "pending"
    } else if external(reason(request)) {
        "external"
    } else {
        "bus"
    };
    Outcome {
        delivery,
        capture,
        cause,
    }
}
fn external(reason: &str) -> bool {
    matches!(
        reason,
        "agent_deleted" | "room_deleted" | "interrupted" | "provider_error"
    ) || reason.starts_with("provider_error:")
}
#[derive(Default)]
struct SeriesBuilder {
    counts: Counts,
    hours: BTreeMap<i64, (String, Counts)>,
}
impl SeriesBuilder {
    fn add(&mut self, hour: &HourKey, request: &Request, outcome: &Outcome) {
        self.counts.add(request, outcome);
        self.hours
            .entry(hour.start_ms)
            .or_insert_with(|| (hour.label.clone(), Counts::default()))
            .1
            .add(request, outcome);
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
pub(super) fn snapshot(state: &BusState, clock: impl Fn(u64) -> HourKey) -> Dashboard {
    let mut all = SeriesBuilder::default();
    let mut providers: BTreeMap<_, _> = ["claude", "codex", "kimi", "grok", "cursor"]
        .into_iter()
        .map(|p| (p.to_owned(), SeriesBuilder::default()))
        .collect();
    for request in state.requests() {
        let provider = state
            .agent(request.agent_id)
            .map(|a| a.provider)
            .or_else(|| {
                state
                    .retired_agent(request.agent_id)
                    .and_then(|a| a.provider)
            })
            .map_or("unknown", |p| p.label());
        let hour = clock(request.prompt.submitted_at_ms);
        let outcome = classify(state, request);
        all.add(&hour, request, &outcome);
        providers
            .entry(provider.into())
            .or_default()
            .add(&hour, request, &outcome);
    }
    Dashboard {
        all: all.finish(),
        providers: providers
            .into_iter()
            .map(|(p, s)| (p, s.finish()))
            .collect(),
    }
}
fn hour_with_offset(ms: u64, offset_seconds: i32) -> Option<HourKey> {
    let offset = time::UtcOffset::from_whole_seconds(offset_seconds).ok()?;
    let local = time::OffsetDateTime::from_unix_timestamp(i64::try_from(ms / 1000).ok()?)
        .ok()?
        .to_offset(offset);
    let start = local.replace_time(time::Time::from_hms(local.hour(), 0, 0).ok()?);
    Some(HourKey {
        start_ms: start.unix_timestamp() * 1000,
        label: format!(
            "{} {:02}:00 UTC{}{:02}:{:02}",
            local.date(),
            local.hour(),
            if offset_seconds < 0 { "-" } else { "+" },
            offset_seconds.abs() / 3600,
            offset_seconds.abs() % 3600 / 60
        ),
    })
}
pub(super) fn local_hour(ms: u64) -> HourKey {
    let key = i64::try_from(ms / 1000).ok().and_then(|seconds| {
        let local = crate::platform::local_datetime_at(seconds)?;
        let offset = i32::try_from(local.assume_utc().unix_timestamp() - seconds).ok()?;
        hour_with_offset(ms, offset)
    });
    key.unwrap_or_else(|| HourKey {
        start_ms: i64::MAX,
        label: "Unknown timestamp".into(),
    })
}
#[cfg(test)]
#[path = "tests/metrics_test.rs"]
mod tests;
