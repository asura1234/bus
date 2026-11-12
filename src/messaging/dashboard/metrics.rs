//! Read-only delivery metrics over the same live/archive requests as message status.
use crate::messaging::model::{AgentId, BusState, Request, RequestPhase, RoomId};
use serde::Serialize;
use std::collections::BTreeMap;

pub(super) const PROVIDERS: [&str; 6] = ["claude", "claude-ds", "codex", "kimi", "grok", "cursor"];

#[derive(Debug, Default, Serialize)]
struct Counts {
    attempts: u64,
    delivered: u64,
    failed: u64,
    open: u64,
    replied: u64,
    /// Turns that ended with no explicit reply. Delivered success, not a missing reply.
    ended: u64,
    reply_not_captured: u64,
    reply_pending: u64,
    bus_failures: u64,
    external: u64,
    /// Delivery-stage failures only. A delivered message with no reply is not here.
    delivery_bus: u64,
    delivery_external: u64,
    /// Earliest still-open send time, so the page can show the oldest pending age.
    oldest_open_ms: Option<u64>,
    failure_reasons: BTreeMap<String, u64>,
    bus_reasons: BTreeMap<String, u64>,
    external_reasons: BTreeMap<String, u64>,
    delivery_bus_reasons: BTreeMap<String, u64>,
    delivery_external_reasons: BTreeMap<String, u64>,
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
                match outcome.cause {
                    "bus" => {
                        self.delivery_bus += 1;
                        increment(&mut self.delivery_bus_reasons, reason(request));
                    }
                    "external" => {
                        self.delivery_external += 1;
                        increment(&mut self.delivery_external_reasons, reason(request));
                    }
                    _ => {}
                }
            }
            _ => {
                self.open += 1;
                let sent = request.prompt.submitted_at_ms;
                self.oldest_open_ms = Some(
                    self.oldest_open_ms
                        .map_or(sent, |current| current.min(sent)),
                );
            }
        }
        match outcome.capture {
            "replied" => self.replied += 1,
            "missing" => self.reply_not_captured += 1,
            "pending" if outcome.delivery == "delivered" => self.reply_pending += 1,
            "none" if outcome.cause == "success" => self.ended += 1,
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
        if request.phase == RequestPhase::Abandoned && outcome.capture != "replied" {
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
    identity: String,
    /// Send accepted until agent pickup. Empty stays None.
    /// Not computed from reply capture or reply delivery.
    accepted_to_picked: super::latency::Percentiles,
    accepted_to_eligible: super::latency::Percentiles,
    eligible_to_picked: super::latency::Percentiles,
    accepted_to_native_confirmed: super::latency::Percentiles,
    picked_to_native_confirmed: super::latency::Percentiles,
}
#[derive(Default, Serialize)]
struct Rates {
    delivery: Metric,
    /// Same rate as `delivery`: delivered ÷ (delivered + Bus-caused delivery failures).
    delivery_share: Metric,
    bus_reliability: Metric,
}
#[derive(Default, Serialize)]
pub(super) struct Metric {
    numerator: u64,
    denominator: u64,
    percent: Option<f64>,
    /// Conclusive fraction plus the attempts that fraction leaves out.
    /// Launch rates leave this empty, and an empty caption is omitted so their
    /// JSON stays numerator, denominator, and percent.
    #[serde(skip_serializing_if = "String::is_empty")]
    caption: String,
}
fn pending_attempts(counts: &Counts) -> u64 {
    counts.open + counts.reply_pending
}
fn percent_label(percent: f64) -> String {
    let tenths = (percent * 10.0).round() as i64;
    if tenths.rem_euclid(10) == 0 {
        format!("{}%", tenths.div_euclid(10))
    } else {
        format!("{}.{}%", tenths.div_euclid(10), tenths.rem_euclid(10))
    }
}
fn count_label(count: u64, singular: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {singular}s")
    }
}
fn delivery_rate(counts: &Counts) -> Metric {
    let numerator = counts.delivered;
    let denominator = counts.delivered + counts.delivery_bus;
    let percent = rate(numerator, denominator);
    let bus = count_label(counts.delivery_bus, "Bus failure");
    let external = count_label(counts.delivery_external, "external failure");
    let caption = match percent {
        Some(value) => format!(
            "{} of {numerator}/{denominator} delivered, {bus}, {external}",
            percent_label(value)
        ),
        None if counts.attempts == 0 => "—".to_string(),
        None => format!("— · {bus}, {external}"),
    };
    Metric {
        numerator,
        denominator,
        percent,
        caption,
    }
}
fn bus_reliability(counts: &Counts) -> Metric {
    // Explicit replies and turns that ended with no reply are both successful
    // terminal deliveries. Captured finals are not replies.
    let successes = counts.replied + counts.ended;
    conclusive_metric(
        successes,
        successes + counts.bus_failures,
        &[
            ("pending", pending_attempts(counts)),
            ("external", counts.external),
        ],
    )
}
fn attempt_identity(counts: &Counts) -> String {
    let caption = bus_reliability(counts).caption;
    let bus_rate = if counts.replied + counts.ended + counts.bus_failures == 0 {
        format!("Bus reliability is {caption}.")
    } else {
        format!("Of conclusive deliveries, {caption}.")
    };
    format!(
        "{attempts} attempted = {delivered} delivered + {pending} pending + {bus} Bus failures + {external} external. Delivery is {share}. {bus_rate}",
        attempts = counts.attempts,
        delivered = counts.delivered,
        pending = counts.open,
        bus = counts.delivery_bus,
        external = counts.delivery_external,
        share = delivery_rate(counts).caption,
    )
}
fn conclusive_metric(numerator: u64, denominator: u64, excluded: &[(&str, u64)]) -> Metric {
    let percent = rate(numerator, denominator);
    let listed = excluded
        .iter()
        .map(|(name, count)| format!("{count} {name}"))
        .collect::<Vec<_>>()
        .join(" · ");
    let caption = match percent {
        Some(value) => format!(
            "{} of {numerator}/{denominator} conclusive · {listed}",
            percent_label(value)
        ),
        None => format!("— · {listed}"),
    };
    Metric {
        numerator,
        denominator,
        percent,
        caption,
    }
}
impl Metric {
    pub(super) fn new(numerator: u64, denominator: u64) -> Self {
        Self {
            numerator,
            denominator,
            percent: rate(numerator, denominator),
            caption: String::new(),
        }
    }
}
impl From<&Counts> for Rates {
    fn from(counts: &Counts) -> Self {
        Self {
            delivery: delivery_rate(counts),
            delivery_share: delivery_rate(counts),
            bus_reliability: bus_reliability(counts),
        }
    }
}
#[derive(Serialize)]
struct Hour {
    hour: String,
    start_ms: i64,
    counts: Counts,
    rates: Rates,
    identity: String,
}
#[derive(Default, Serialize)]
pub(super) struct Dashboard {
    all: Series,
    providers: BTreeMap<String, Series>,
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
#[cfg(test)]
pub(super) use super::clock::hour_with_offset;
pub(super) use super::clock::{local_hour, HourKey};
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
fn explicit_reply(request: &Request) -> bool {
    request
        .reply()
        .is_some_and(|reply| reply.callback_id.starts_with("reply-to:"))
}

fn classify(state: &BusState, request: &Request) -> Outcome {
    // Only an explicit `bus send --reply-to` counts as a reply. A turn that
    // ends without one is delivered success, not a missing reply.
    if explicit_reply(request) {
        return Outcome {
            delivery: "delivered",
            capture: "replied",
            cause: "success",
        };
    }
    if request.phase == RequestPhase::Completed {
        return Outcome {
            delivery: "delivered",
            capture: "none",
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
    accepted_to_picked: Vec<u64>,
    accepted_to_eligible: Vec<u64>,
    eligible_to_picked: Vec<u64>,
    accepted_to_native_confirmed: Vec<u64>,
    picked_to_native_confirmed: Vec<u64>,
}
impl SeriesBuilder {
    fn add(&mut self, hour: &HourKey, request: &Request, outcome: &Outcome) {
        if let Some(ms) = super::latency::accepted_to_eligible_ms(request) {
            self.accepted_to_eligible.push(ms);
        }
        if let Some(ms) = super::latency::eligible_to_picked_ms(request) {
            self.eligible_to_picked.push(ms);
        }
        if let Some(ms) = super::latency::accepted_to_native_confirmed_ms(request) {
            self.accepted_to_native_confirmed.push(ms);
        }
        if let Some(ms) = super::latency::picked_to_native_confirmed_ms(request) {
            self.picked_to_native_confirmed.push(ms);
        }
        self.counts.add(request, outcome);
        self.hours
            .entry(hour.start_ms)
            .or_insert_with(|| (hour.label.clone(), Counts::default()))
            .1
            .add(request, outcome);
        if let Some(ms) = super::latency::accepted_to_picked_ms(request) {
            self.accepted_to_picked.push(ms);
        }
    }
    fn finish(self) -> Series {
        let rates = Rates::from(&self.counts);
        let identity = attempt_identity(&self.counts);
        Series {
            counts: self.counts,
            rates,
            identity,
            accepted_to_picked: super::latency::percentiles(&self.accepted_to_picked),
            accepted_to_eligible: super::latency::percentiles(&self.accepted_to_eligible),
            eligible_to_picked: super::latency::percentiles(&self.eligible_to_picked),
            accepted_to_native_confirmed: super::latency::percentiles(
                &self.accepted_to_native_confirmed,
            ),
            picked_to_native_confirmed: super::latency::percentiles(
                &self.picked_to_native_confirmed,
            ),
            hours: self
                .hours
                .into_iter()
                .map(|(start_ms, (hour, counts))| {
                    let rates = Rates::from(&counts);
                    let identity = attempt_identity(&counts);
                    Hour {
                        hour,
                        start_ms,
                        counts,
                        rates,
                        identity,
                    }
                })
                .collect(),
        }
    }
}
pub(super) fn provider_label(state: &BusState, id: AgentId) -> &'static str {
    state
        .agent(id)
        .map(|agent| agent.provider)
        .or_else(|| state.retired_agent(id).and_then(|agent| agent.provider))
        .map_or("unknown", |provider| provider.label())
}

pub(super) fn room_label(state: &BusState, id: RoomId) -> String {
    state
        .history_room_name(id)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("Room {} [deleted]", id.0))
}

pub(super) fn snapshot(state: &BusState, clock: impl Fn(u64) -> HourKey) -> Dashboard {
    let mut all = SeriesBuilder::default();
    let mut providers: BTreeMap<_, _> = PROVIDERS
        .into_iter()
        .map(|p| (p.to_owned(), SeriesBuilder::default()))
        .collect();
    let mut rooms: BTreeMap<String, GroupBuilder> = BTreeMap::new();
    let mut failures = Vec::new();
    for request in state.requests() {
        // Deliberate closures remain in message history, outside every dashboard
        // population (including delivery rates and latency samples). Successful
        // replies/turns still win over stale closure reasons.
        if request.phase == RequestPhase::Abandoned
            && !explicit_reply(request)
            && crate::messaging::model::classify_request_failure(reason(request)).is_deliberate()
        {
            continue;
        }
        let provider = provider_label(state, request.agent_id);
        let hour = clock(request.prompt.submitted_at_ms);
        let outcome = classify(state, request);
        let name = state.agent_label(request.agent_id);
        let room = room_label(state, request.room_id);
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
#[cfg(test)]
#[path = "tests/metrics_test.rs"]
mod tests;
