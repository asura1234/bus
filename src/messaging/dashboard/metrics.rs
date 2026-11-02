//! Read-only delivery metrics over the same live/archive requests as message status.
use crate::messaging::model::{BusState, Request, RequestPhase};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Default, Serialize)]
struct Counts {
    attempts: u64,
    delivered: u64,
    failed: u64,
    open: u64,
    replied: u64,
    reply_not_captured: u64,
    reply_pending: u64,
    bus_failures: u64,
    external: u64,
    failure_reasons: BTreeMap<String, u64>,
    bus_reasons: BTreeMap<String, u64>,
    external_reasons: BTreeMap<String, u64>,
    terminal_reasons: BTreeMap<String, u64>,
    stages: BTreeMap<String, u64>,
}
impl Counts {
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
        increment(&mut self.stages, request.delivery_stage());
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
#[derive(Default, Serialize)]
struct Series {
    counts: Counts,
    hours: Vec<Hour>,
    rates: Rates,
}
#[derive(Default, Serialize)]
struct Rates {
    delivery: Metric,
    reply_capture: Metric,
    bus_reliability: Metric,
}
#[derive(Default, Serialize)]
struct Metric {
    numerator: u64,
    denominator: u64,
    percent: Option<f64>,
}
impl Metric {
    fn new(numerator: u64, denominator: u64) -> Self {
        Self {
            numerator,
            denominator,
            percent: rate(numerator, denominator),
        }
    }
}
impl From<&Counts> for Rates {
    fn from(c: &Counts) -> Self {
        Self {
            delivery: Metric::new(c.delivered, c.delivered + c.failed),
            reply_capture: Metric::new(c.replied, c.replied + c.reply_not_captured),
            bus_reliability: Metric::new(c.replied, c.replied + c.bus_failures),
        }
    }
}
#[derive(Serialize)]
struct Hour {
    hour: String,
    start_ms: i64,
    counts: Counts,
    rates: Rates,
}
#[derive(Default, Serialize)]
pub(super) struct Dashboard {
    all: Series,
    providers: BTreeMap<String, Series>,
    recipients: BTreeMap<String, Group>,
    rooms: BTreeMap<String, Group>,
    failures: Vec<Failure>,
}
#[derive(Serialize)]
struct Group {
    name: String,
    provider: Option<String>,
    series: Series,
}
#[derive(Serialize)]
struct Failure {
    request_id: u64,
    message_id: u64,
    recipient_id: u64,
    recipient: String,
    provider: &'static str,
    room_id: u64,
    room: String,
    sent_at_ms: u64,
    hour_start_ms: i64,
    hour: String,
    stage: &'static str,
    reason: String,
    delivery: &'static str,
    cause: &'static str,
}
#[derive(Default)]
struct GroupBuilder {
    name: String,
    provider: Option<String>,
    series: SeriesBuilder,
}
impl GroupBuilder {
    fn finish(self) -> Group {
        Group {
            name: self.name,
            provider: self.provider,
            series: self.series.finish(),
        }
    }
}
#[derive(Clone)]
pub(super) struct HourKey {
    start_ms: i64,
    pub(super) label: String,
}
struct Outcome {
    delivery: &'static str,
    capture: &'static str,
    cause: &'static str,
}
fn pre_start_failure(request: &Request) -> bool {
    crate::messaging::model::request_pre_start_failure(request)
}
fn bound(state: &BusState, request: &Request) -> bool {
    crate::messaging::model::request_start_bound(state, request)
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
    crate::messaging::model::classify_request_failure(reason).is_external()
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
    let mut recipients: BTreeMap<String, GroupBuilder> = BTreeMap::new();
    let mut rooms: BTreeMap<String, GroupBuilder> = BTreeMap::new();
    let mut failures = Vec::new();
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
        let name = state.agent_label(request.agent_id);
        let room = state
            .history_room_name(request.room_id)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Room {} [deleted]", request.room_id.0));
        recipients
            .entry(request.agent_id.0.to_string())
            .or_insert_with(|| GroupBuilder {
                name: name.clone(),
                provider: Some(provider.into()),
                ..Default::default()
            })
            .series
            .add(&hour, request, &outcome);
        rooms
            .entry(request.room_id.0.to_string())
            .or_insert_with(|| GroupBuilder {
                name: room.clone(),
                ..Default::default()
            })
            .series
            .add(&hour, request, &outcome);
        if matches!(outcome.cause, "bus" | "external") {
            failures.push(Failure {
                request_id: request.id.0,
                message_id: request.prompt.id.0,
                recipient_id: request.agent_id.0,
                recipient: name,
                provider,
                room_id: request.room_id.0,
                room,
                sent_at_ms: request.prompt.submitted_at_ms,
                hour_start_ms: hour.start_ms,
                hour: hour.label.clone(),
                stage: request.delivery_stage(),
                reason: reason(request).to_owned(),
                delivery: outcome.delivery,
                cause: outcome.cause,
            });
        }
        all.add(&hour, request, &outcome);
        providers
            .entry(provider.into())
            .or_default()
            .add(&hour, request, &outcome);
    }
    Dashboard {
        all: all.finish(),
        recipients: recipients
            .into_iter()
            .map(|(id, group)| (id, group.finish()))
            .collect(),
        rooms: rooms
            .into_iter()
            .map(|(id, group)| (id, group.finish()))
            .collect(),
        failures,
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
