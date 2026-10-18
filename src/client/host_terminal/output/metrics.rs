//! Numeric aggregates only; the existing perf recorder drains these atomics.
use crate::utils::perf::{Counter, Gauge, Timer};

pub(super) static OUTPUT: OutputMetrics = OutputMetrics::new();

pub(super) struct OutputMetrics {
    pub(super) frame: Operation,
    pub(super) patch: Operation,
    pub(super) coalesced: Counter,
    pub(super) pending_age_ms: Gauge,
}

pub(super) struct Operation {
    pub(super) encode: Timer,
    pub(super) write: Timer,
    pub(super) flush: Timer,
    /// Encoded bytes produced, including graphics save/restore framing, even
    /// if a later host write fails. No scan or copy of the payload is needed.
    pub(super) encoded_bytes: Counter,
}

impl OutputMetrics {
    pub(super) const fn new() -> Self {
        Self {
            frame: Operation {
                encode: Timer::new("client.output.frame.encode"),
                write: Timer::new("client.output.frame.write"),
                flush: Timer::new("client.output.frame.flush"),
                encoded_bytes: Counter::new("client.output.frame.encoded_bytes"),
            },
            patch: Operation {
                encode: Timer::new("client.output.patch.encode"),
                write: Timer::new("client.output.patch.write"),
                flush: Timer::new("client.output.patch.flush"),
                encoded_bytes: Counter::new("client.output.patch.encoded_bytes"),
            },
            coalesced: Counter::new("client.output.frames_coalesced"),
            pending_age_ms: Gauge::new("client.output.pending_age_ms"),
        }
    }
}
