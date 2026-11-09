//! Read-only Bus-overhead latency metrics over live and archived requests.
use crate::messaging::model::{BusState, Request};
use serde::Serialize;
use std::collections::BTreeMap;

const PHASES: &[&str] = &[
    "accepted_to_box",
    "accepted_to_picked",
    "enqueue",
    "ready_queue",
    "dispatch",
    "native_transport",
    "writer_queue",
    "paste_prepare",
    "text_write",
    "paste_pacing",
    "ownership_wait",
    "enter_dispatch",
    "enter_write",
    "post_enter_confirmation",
    "native_total",
    "end_to_end",
];

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub(super) struct Percentiles {
    pub(super) n: u64,
    pub(super) p50: Option<u64>,
    pub(super) p90: Option<u64>,
    pub(super) p99: Option<u64>,
    pub(super) max: Option<u64>,
    /// Nearest millisecond. Absent when `n` is 0, so an empty group is not 0.
    pub(super) mean: Option<u64>,
}

pub(super) fn percentiles(samples: &[u64]) -> Percentiles {
    let mean = (!samples.is_empty()).then(|| {
        let sum: u128 = samples.iter().map(|sample| u128::from(*sample)).sum();
        let count = samples.len() as u128;
        ((sum + count / 2) / count) as u64
    });
    let mut samples = samples.to_vec();
    samples.sort_unstable();
    let at = |percent: f64| {
        (!samples.is_empty()).then(|| samples[(samples.len() as f64 * percent).ceil() as usize - 1])
    };
    Percentiles {
        n: samples.len() as u64,
        p50: at(0.50),
        p90: at(0.90),
        p99: at(0.99),
        max: samples.last().copied(),
        mean,
    }
}

/// End-to-end delay to room delivery. A missing or reversed pair contributes nothing.
pub(super) fn end_to_end_ms(request: &Request) -> Option<u64> {
    aggregate(request, "end_to_end")
}

/// Send accepted until the agent turn takes the message. A missing or reversed pair contributes nothing.
pub(super) fn accepted_to_picked_ms(request: &Request) -> Option<u64> {
    aggregate(request, "accepted_to_picked")
}

pub(super) fn elapsed(start: Option<u64>, end: Option<u64>) -> Option<u64> {
    end?.checked_sub(start?)
}

/// A native stamp counts only when it is inside the current `native_requested` attempt.
fn attempt_stamp(anchor: Option<u64>, stamp: Option<u64>) -> Option<u64> {
    let anchor = anchor?;
    let stamp = stamp?;
    (stamp >= anchor).then_some(stamp)
}

fn ready(request: &Request) -> bool {
    request.latency.ready_at_enqueue && !request.latency.agent_waited
}

fn phase(request: &Request, name: &str) -> Option<u64> {
    let latency = &request.latency;
    let native = latency.native;
    let anchor = latency.native_requested_at_ms;
    let native_at = |stamp| attempt_stamp(anchor, stamp);
    let pair = |start, end| elapsed(start, end);
    match name {
        "accepted_to_box" => pair(latency.accepted_at_ms, native_at(native.text_written_at_ms)),
        "accepted_to_picked" => pair(latency.accepted_at_ms, latency.picked_at_ms),
        "enqueue" => pair(latency.accepted_at_ms, latency.queued_at_ms),
        "ready_queue" => pair(latency.queued_at_ms, latency.picked_at_ms),
        "dispatch" => pair(latency.picked_at_ms, anchor),
        "native_transport" => pair(anchor, native_at(native.server_accepted_at_ms)),
        "writer_queue" => pair(
            native_at(native.server_accepted_at_ms),
            native_at(native.writer_queued_at_ms),
        ),
        "paste_prepare" => pair(
            native_at(native.writer_queued_at_ms),
            native_at(native.paste_started_at_ms),
        ),
        "text_write" => pair(
            native_at(native.paste_started_at_ms),
            native_at(native.text_written_at_ms),
        ),
        "paste_pacing" => pair(
            native_at(native.text_written_at_ms),
            native_at(native.paste_completed_at_ms),
        ),
        "ownership_wait" => pair(
            native_at(native.paste_completed_at_ms),
            native_at(native.composer_owned_at_ms),
        ),
        "enter_dispatch" => pair(
            native_at(native.composer_owned_at_ms),
            native_at(native.enter_sent_at_ms),
        ),
        "enter_write" => pair(
            native_at(native.enter_sent_at_ms),
            native_at(native.enter_written_at_ms),
        ),
        "post_enter_confirmation" => pair(
            native_at(native.enter_written_at_ms),
            native_at(native.native_confirmed_at_ms),
        ),
        "native_total" => pair(anchor, native_at(native.native_confirmed_at_ms)),
        "end_to_end" => pair(latency.accepted_at_ms, latency.reply_delivered_at_ms),
        "queue_total" => pair(latency.queued_at_ms, latency.picked_at_ms),
        _ => None,
    }
}

fn aggregate(request: &Request, name: &str) -> Option<u64> {
    (name != "accepted_to_box" && name != "ready_queue" || ready(request))
        .then(|| phase(request, name))
        .flatten()
}

#[derive(Default, Serialize)]
pub(super) struct Series {
    pub(super) phases: BTreeMap<&'static str, Percentiles>,
    pub(super) hours: Vec<Hour>,
}

#[derive(Serialize)]
pub(super) struct Hour {
    pub(super) hour: String,
    pub(super) start_ms: i64,
    pub(super) phases: BTreeMap<&'static str, Percentiles>,
}

#[derive(Serialize)]
pub(super) struct Group {
    pub(super) name: String,
    pub(super) provider: Option<String>,
    pub(super) series: Series,
}

#[derive(Clone, Serialize)]
pub(super) struct SlowRequest {
    pub(super) request_id: u64,
    pub(super) message_id: u64,
    pub(super) recipient_id: u64,
    pub(super) recipient: String,
    pub(super) provider: &'static str,
    pub(super) room_id: u64,
    pub(super) room: String,
    pub(super) sent_at_ms: u64,
    pub(super) hour_start_ms: i64,
    pub(super) accepted_to_box_ms: Option<u64>,
    pub(super) phases: BTreeMap<&'static str, u64>,
}

#[derive(Default, Serialize)]
pub(super) struct Dashboard {
    pub(super) all: Series,
    pub(super) providers: BTreeMap<String, Series>,
    pub(super) recipients: BTreeMap<String, Group>,
    pub(super) slowest: Vec<SlowRequest>,
    pub(super) provider_slowest: BTreeMap<String, Vec<SlowRequest>>,
    pub(super) recipient_slowest: BTreeMap<String, Vec<SlowRequest>>,
}

#[derive(Default)]
struct SeriesBuilder {
    samples: BTreeMap<&'static str, Vec<u64>>,
    hours: BTreeMap<i64, (String, BTreeMap<&'static str, Vec<u64>>)>,
}

impl SeriesBuilder {
    fn add(&mut self, hour: &HourKey, request: &Request) {
        for name in PHASES {
            if let Some(value) = aggregate(request, name) {
                self.samples.entry(name).or_default().push(value);
                self.hours
                    .entry(hour.start_ms)
                    .or_insert_with(|| (hour.label.clone(), BTreeMap::default()))
                    .1
                    .entry(name)
                    .or_default()
                    .push(value);
            }
        }
    }

    fn finish(self) -> Series {
        Series {
            phases: PHASES
                .iter()
                .map(|name| {
                    (
                        *name,
                        percentiles(self.samples.get(name).map_or(&[], Vec::as_slice)),
                    )
                })
                .collect(),
            hours: self
                .hours
                .into_iter()
                .map(|(start_ms, (hour, samples))| Hour {
                    hour,
                    start_ms,
                    phases: PHASES
                        .iter()
                        .map(|name| {
                            (
                                *name,
                                percentiles(samples.get(name).map_or(&[], Vec::as_slice)),
                            )
                        })
                        .collect(),
                })
                .collect(),
        }
    }
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
pub(super) use super::clock::HourKey;

pub(super) fn snapshot(state: &BusState, clock: impl Fn(u64) -> HourKey) -> Dashboard {
    let mut all = SeriesBuilder::default();
    let mut providers: BTreeMap<_, _> = ["claude", "codex", "kimi", "grok", "cursor"]
        .into_iter()
        .map(|provider| (provider.to_owned(), SeriesBuilder::default()))
        .collect();
    let mut recipients: BTreeMap<String, GroupBuilder> = BTreeMap::new();
    let mut slowest = Vec::new();
    let mut provider_slowest: BTreeMap<String, Vec<SlowRequest>> = BTreeMap::new();
    let mut recipient_slowest: BTreeMap<String, Vec<SlowRequest>> = BTreeMap::new();
    for request in state.requests() {
        let provider = super::metrics::provider_label(state, request.agent_id);
        let hour = clock(
            request
                .latency
                .accepted_at_ms
                .unwrap_or(request.prompt.submitted_at_ms),
        );
        all.add(&hour, request);
        providers
            .entry(provider.into())
            .or_default()
            .add(&hour, request);
        recipients
            .entry(request.agent_id.0.to_string())
            .or_insert_with(|| GroupBuilder {
                name: state.agent_label(request.agent_id),
                provider: Some(provider.into()),
                ..Default::default()
            })
            .series
            .add(&hour, request);
        if ready(request) {
            if let Some(total) = phase(request, "accepted_to_box") {
                let mut phases = BTreeMap::new();
                for name in PHASES.iter().copied().chain(["queue_total"]) {
                    if let Some(value) = phase(request, name) {
                        phases.insert(name, value);
                    }
                }
                let slow = SlowRequest {
                    request_id: request.id.0,
                    message_id: request.prompt.id.0,
                    recipient_id: request.agent_id.0,
                    recipient: state.agent_label(request.agent_id),
                    provider,
                    room_id: request.room_id.0,
                    room: super::metrics::room_label(state, request.room_id),
                    sent_at_ms: request.prompt.submitted_at_ms,
                    hour_start_ms: hour.start_ms,
                    accepted_to_box_ms: Some(total),
                    phases,
                };
                provider_slowest
                    .entry(provider.into())
                    .or_default()
                    .push(slow.clone());
                recipient_slowest
                    .entry(request.agent_id.0.to_string())
                    .or_default()
                    .push(slow.clone());
                slowest.push(slow);
            }
        }
    }
    sort_slowest(&mut slowest);
    for requests in provider_slowest
        .values_mut()
        .chain(recipient_slowest.values_mut())
    {
        sort_slowest(requests);
    }
    Dashboard {
        all: all.finish(),
        providers: providers
            .into_iter()
            .map(|(provider, series)| (provider, series.finish()))
            .collect(),
        recipients: recipients
            .into_iter()
            .map(|(id, group)| (id, group.finish()))
            .collect(),
        slowest,
        provider_slowest,
        recipient_slowest,
    }
}

fn sort_slowest(requests: &mut Vec<SlowRequest>) {
    requests.sort_by_key(|request| {
        std::cmp::Reverse((request.accepted_to_box_ms.unwrap_or(0), request.request_id))
    });
    requests.truncate(50);
}
