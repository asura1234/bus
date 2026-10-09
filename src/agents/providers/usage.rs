//! Provider-neutral quota windows and opaque observation routing facts.
//! Room-agent attribution and freshness policy belong to the coordinator.
use serde::{Deserialize, Serialize};

use super::ProviderKind;

pub(crate) const FIVE_HOUR_MINUTES: u64 = 300;
pub(crate) const WEEKLY_MINUTES: u64 = 7 * 24 * 60;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct UsageWindow {
    pub(crate) used_percent: f64,
    /// Unix seconds, as the provider reports it.
    pub(crate) resets_at: Option<u64>,
    pub(crate) window_minutes: u64,
}

/// Windows the provider reported. A `None` window was absent, so unknown.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct UsageWindows {
    pub(crate) five_hour: Option<UsageWindow>,
    pub(crate) weekly: Option<UsageWindow>,
}

/// Existing serialized manifest shape; the numeric token has no room semantics here.
#[derive(Deserialize, Serialize)]
pub(crate) struct ObservationManifest {
    pub(crate) agent_id: u64,
    pub(crate) provider: ProviderKind,
    pub(crate) launch_id: String,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct Observation {
    pub(crate) manifest: ObservationManifest,
    pub(crate) session_id: String,
    pub(crate) read_at_ms: u64,
    pub(crate) windows: UsageWindows,
}
