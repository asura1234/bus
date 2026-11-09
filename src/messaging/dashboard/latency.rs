//! Pickup latency: send accepted until the agent turn takes the message.
use crate::messaging::model::Request;
use serde::Serialize;

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

/// Send accepted until the agent turn takes the message. A missing or reversed pair contributes nothing.
pub(super) fn accepted_to_picked_ms(request: &Request) -> Option<u64> {
    elapsed(request.latency.accepted_at_ms, request.latency.picked_at_ms)
}

/// Ready-queue time for a message that waited for an idle agent. A steer joins
/// a running turn, so it has no ready-queue sample.
pub(crate) fn ready_queue_sample(request: &Request) -> Option<u64> {
    let latency = &request.latency;
    (latency.ready_at_enqueue && !latency.agent_waited && !request.steered)
        .then(|| elapsed(latency.queued_at_ms, latency.picked_at_ms))
        .flatten()
}

pub(super) fn elapsed(start: Option<u64>, end: Option<u64>) -> Option<u64> {
    end?.checked_sub(start?)
}
