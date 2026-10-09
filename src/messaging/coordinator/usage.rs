//! Newest per-login quota snapshots for the dev `state` command.
//! Parsing lives below messaging; binding, throttling and attribution stay here.
use std::{
    collections::BTreeMap,
    path::Path,
    time::{Duration, Instant},
};

use serde_json::{json, Value};

use crate::agents::providers::{
    claude_code::statusline as claude_statusline,
    usage::{Observation, UsageWindow, UsageWindows},
    ProviderKind,
};
use crate::messaging::model::{AgentId, Provider, RoomAgent};
use crate::utils::time as bus_io;

const STALE_AFTER_MS: u64 = 15 * 60 * 1000;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct UsageSnapshot {
    pub(crate) windows: UsageWindows,
    pub(crate) read_at_ms: u64,
    pub(crate) observed_by_agent: AgentId,
}

#[derive(Debug)]
pub(crate) struct Usage {
    pub(crate) codex: Option<UsageSnapshot>,
    pub(crate) claude: Option<UsageSnapshot>,
    started_at_ms: u64,
    claude_reads: BTreeMap<AgentId, Instant>,
}

impl Default for Usage {
    fn default() -> Self {
        Self {
            codex: None,
            claude: None,
            started_at_ms: bus_io::now_ms(),
            claude_reads: BTreeMap::new(),
        }
    }
}

impl Usage {
    pub(crate) fn state_json(&self) -> Value {
        json!({
            "claude": self.claude_json(bus_io::now_ms()),
            "codex": match &self.codex {
                None => unknown("No Codex turn has reported rate limits since Bus started"),
                Some(snapshot) => json!({
                    "status": "observed",
                    "five_hour": snapshot.windows.five_hour,
                    "weekly": snapshot.windows.weekly,
                    "read_at_ms": snapshot.read_at_ms,
                    "observed_by_agent": snapshot.observed_by_agent,
                }),
            },
            "cursor": unknown("Cursor has no local allowance source"),
        })
    }

    fn claude_json(&self, now_ms: u64) -> Value {
        let Some(snapshot) = &self.claude else {
            return unknown("No Claude status line has reported rate limits since Bus started");
        };
        if snapshot.read_at_ms > now_ms || now_ms - snapshot.read_at_ms >= STALE_AFTER_MS {
            return unknown("Claude allowance observation is stale");
        }
        let current = |w: &&UsageWindow| w.resets_at.is_none_or(|at| at > now_ms / 1000);
        let five_hour = snapshot.windows.five_hour.as_ref().filter(current);
        let weekly = snapshot.windows.weekly.as_ref().filter(current);
        if five_hour.is_none() && weekly.is_none() {
            return unknown("Claude allowance windows are missing or expired");
        }
        json!({"status":"observed", "five_hour":five_hour, "weekly":weekly,
            "read_at_ms":snapshot.read_at_ms, "observed_by_agent":snapshot.observed_by_agent})
    }

    /// Advisory only: no State mutation, delivery errors, or event-spool growth.
    pub(crate) fn refresh_claude(&mut self, agent: &RoomAgent, spool: &Path) {
        if agent.provider != Provider::ClaudeCode || agent.session_binding_invalidated {
            return;
        }
        let now = Instant::now();
        if self
            .claude_reads
            .get(&agent.id)
            .is_some_and(|last| now.duration_since(*last) < Duration::from_secs(1))
        {
            return;
        }
        self.claude_reads.insert(agent.id, now);
        let Ok(record) = claude_statusline::read_usage(spool) else {
            return;
        };
        if record.manifest.agent_id != agent.id.0
            || record.manifest.provider != ProviderKind::ClaudeCode
            || Some(record.manifest.launch_id.as_str())
                != agent.runtime_identity.launch_id.as_deref()
            || Some(record.session_id.as_str()) != agent.runtime_identity.session_id.as_deref()
        {
            return;
        }
        self.merge_claude(record, agent.id);
    }

    fn merge_claude(&mut self, record: Observation, observed_by_agent: AgentId) {
        if record.read_at_ms < self.started_at_ms
            || record.read_at_ms > bus_io::now_ms()
            || self
                .claude
                .as_ref()
                .is_some_and(|old| old.read_at_ms >= record.read_at_ms)
        {
            return;
        }
        self.claude = Some(UsageSnapshot {
            windows: record.windows,
            read_at_ms: record.read_at_ms,
            observed_by_agent,
        });
    }
}

fn unknown(reason: &str) -> Value {
    json!({"status": "unknown", "reason": reason})
}

#[cfg(test)]
#[path = "tests/usage_test.rs"]
mod tests;
