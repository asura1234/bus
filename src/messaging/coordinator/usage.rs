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
use crate::messaging::model::{AgentId, BusState, Provider, RequestId, RoomAgent};
use crate::utils::time as bus_io;

const STALE_AFTER_MS: u64 = 15 * 60 * 1000;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct UsageSnapshot {
    pub(crate) windows: UsageWindows,
    pub(crate) read_at_ms: u64,
    pub(crate) observed_by_agent: AgentId,
}

impl UsageSnapshot {
    fn fresh(&self, now_ms: u64) -> bool {
        self.read_at_ms > 0
            && self.read_at_ms <= now_ms
            && now_ms - self.read_at_ms < STALE_AFTER_MS
    }

    fn current_window(&self, window: &UsageWindow, now_ms: u64) -> bool {
        self.fresh(now_ms) && window.resets_at.is_some_and(|at| at > now_ms / 1000)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CursorUsageSnapshot {
    error: String,
    resets_at: u64,
    read_at_ms: u64,
    observed_by_agent: AgentId,
    request: Option<RequestId>,
}

impl CursorUsageSnapshot {
    fn current(&self, now_ms: u64) -> bool {
        self.read_at_ms > 0
            && self.read_at_ms <= now_ms
            && now_ms - self.read_at_ms < STALE_AFTER_MS
            && self.resets_at > now_ms / 1000
    }
}

#[derive(Debug)]
pub(crate) struct Usage {
    pub(crate) codex: Option<UsageSnapshot>,
    pub(crate) claude: Option<UsageSnapshot>,
    cursor: Option<CursorUsageSnapshot>,
    cursor_errors: BTreeMap<AgentId, CursorUsageSnapshot>,
    started_at_ms: u64,
    claude_reads: BTreeMap<AgentId, Instant>,
}

impl Default for Usage {
    fn default() -> Self {
        Self {
            codex: None,
            claude: None,
            cursor: None,
            cursor_errors: BTreeMap::new(),
            started_at_ms: bus_io::now_ms(),
            claude_reads: BTreeMap::new(),
        }
    }
}

impl Usage {
    /// Only the failing agent's fresh native allowance may defer its quota probe.
    pub(crate) fn retry_reset(&self, agent: &RoomAgent, now_ms: u64) -> Option<u64> {
        let snapshot = match agent.provider {
            Provider::Codex => self.codex.as_ref(),
            Provider::ClaudeCode => self.claude.as_ref(),
            _ => None,
        }?;
        if snapshot.observed_by_agent != agent.id || !snapshot.fresh(now_ms) {
            return None;
        }
        [&snapshot.windows.five_hour, &snapshot.windows.weekly]
            .into_iter()
            .flatten()
            .filter(|window| {
                window.used_percent >= 100.0 && snapshot.current_window(window, now_ms)
            })
            .filter_map(|window| window.resets_at?.checked_mul(1000))
            .max()
    }

    pub(crate) fn exhausted(&self, now_ms: u64) -> Vec<(String, AgentId, Option<bool>)> {
        let mut result = Vec::new();
        for (provider, snapshot) in [("codex", &self.codex), ("claude", &self.claude)] {
            let Some(snapshot) = snapshot else {
                continue;
            };
            for (name, window) in [
                ("five_hour", &snapshot.windows.five_hour),
                ("weekly", &snapshot.windows.weekly),
            ] {
                let active = window
                    .as_ref()
                    .filter(|window| snapshot.current_window(window, now_ms))
                    .map(|window| window.used_percent >= 100.0);
                result.push((
                    format!(
                        "quota:{provider}:{name}:{}",
                        window
                            .as_ref()
                            .and_then(|window| window.resets_at)
                            .unwrap_or(0)
                    ),
                    snapshot.observed_by_agent,
                    active,
                ));
            }
        }
        if let Some(snapshot) = &self.cursor {
            result.push((
                format!("quota:cursor:monthly:{}", snapshot.resets_at),
                snapshot.observed_by_agent,
                snapshot.current(now_ms).then_some(true),
            ));
        }
        result
    }

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
            "cursor": self.cursor_json(bus_io::now_ms()),
            "grok": unknown(crate::agents::providers::grok::usage::allowance_unavailable_reason()),
            "kimi": unknown("Kimi has no local allowance source"),
        })
    }

    fn claude_json(&self, now_ms: u64) -> Value {
        let Some(snapshot) = &self.claude else {
            return unknown("No Claude status line has reported rate limits since Bus started");
        };
        if !snapshot.fresh(now_ms) {
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

    fn cursor_json(&self, now_ms: u64) -> Value {
        let Some(snapshot) = &self.cursor else {
            return unknown("No Cursor usage limit has been observed since Bus started");
        };
        if !snapshot.current(now_ms) {
            return unknown("Cursor usage-limit observation is stale or expired");
        }
        json!({"status":"observed", "monthly":{"used_percent":100.0,"resets_at":snapshot.resets_at},
            "read_at_ms":snapshot.read_at_ms, "observed_by_agent":snapshot.observed_by_agent})
    }

    pub(crate) fn observe_cursor(
        &mut self,
        agent: AgentId,
        error: Option<&str>,
        resets_at: Option<u64>,
        read_at_ms: u64,
        request: Option<RequestId>,
    ) {
        if let (Some(error), Some(resets_at)) = (error, resets_at) {
            let snapshot = CursorUsageSnapshot {
                error: error.to_owned(),
                resets_at,
                read_at_ms,
                observed_by_agent: agent,
                request,
            };
            if request.is_some() {
                self.cursor_errors.insert(agent, snapshot.clone());
            }
            self.cursor = Some(snapshot);
        } else {
            if self
                .cursor
                .as_ref()
                .is_some_and(|snapshot| snapshot.observed_by_agent == agent)
            {
                self.cursor = None;
            }
            self.cursor_errors.remove(&agent);
        }
    }

    pub(crate) fn cursor_error_for_turn(
        &self,
        agent: AgentId,
        state: &BusState,
        session: &str,
        turn: &str,
    ) -> Option<String> {
        let snapshot = self.cursor_errors.get(&agent)?;
        let request_id = state.agent(agent)?.current_request?;
        let request = state.request(request_id)?;
        (snapshot.observed_by_agent == agent
            && snapshot.request == Some(request_id)
            && snapshot.current(bus_io::now_ms())
            && request.trusted_start_bound
            && request.provider_session_id.as_deref() == Some(session)
            && request.provider_turn_id.as_deref() == Some(turn))
        .then(|| snapshot.error.clone())
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
