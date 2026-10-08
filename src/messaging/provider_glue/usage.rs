//! Provider allowance snapshots for the dev `state` command.
//!
//! Every agent on one provider login shares its quota, so Bus keeps the newest
//! snapshot per provider. Snapshots live in the coordinator's memory; Claude's
//! launch spool is a transient input, never a saved-session quota cache.
use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom},
    path::Path,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::messaging::model::{Agent, AgentId, Provider};
use crate::messaging::storage::io as bus_io;

#[path = "claude_statusline.rs"]
pub(crate) mod claude_statusline;

/// Enough rollout tail to reach the last `token_count` event after a turn.
const ROLLOUT_TAIL_BYTES: u64 = 256 * 1024;
const FIVE_HOUR_MINUTES: u64 = 300;
const WEEKLY_MINUTES: u64 = 7 * 24 * 60;
const STALE_AFTER_MS: u64 = 15 * 60 * 1000;

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
    pub(crate) fn refresh_claude(&mut self, agent: &Agent, spool: &Path) {
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
        if record.manifest.agent_id != agent.id
            || record.manifest.provider != agent.provider
            || Some(record.manifest.launch_id.as_str())
                != agent.runtime_identity.launch_id.as_deref()
            || Some(record.session_id.as_str()) != agent.runtime_identity.session_id.as_deref()
        {
            return;
        }
        self.merge_claude(record);
    }

    fn merge_claude(&mut self, record: claude_statusline::Observation) {
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
            observed_by_agent: record.manifest.agent_id,
        });
    }
}

fn unknown(reason: &str) -> Value {
    json!({"status": "unknown", "reason": reason})
}

/// Accept only Codex rollout files: hook payloads name the path, and Bus
/// should not parse arbitrary files a payload points at.
pub(crate) fn codex_rollout_path(value: &Value) -> Option<&Path> {
    let path = Path::new(value.get("transcript_path")?.as_str()?);
    let name = path.file_name()?.to_str()?;
    (path.is_absolute() && name.starts_with("rollout-") && name.ends_with(".jsonl")).then_some(path)
}

/// The newest `token_count` rate limits in a Codex rollout. The format is
/// undocumented: windows are classified by duration because Codex may put
/// the weekly window in either slot. `Ok(None)` means none were found.
pub(crate) fn read_codex_rollout(path: &Path) -> std::io::Result<Option<UsageWindows>> {
    let mut file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    let start = length.saturating_sub(ROLLOUT_TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut tail = Vec::new();
    file.take(ROLLOUT_TAIL_BYTES).read_to_end(&mut tail)?;
    let tail = String::from_utf8_lossy(&tail);
    // A partial first line from the seek simply fails to parse.
    Ok(tail.lines().rev().find_map(codex_rate_limits))
}

fn codex_rate_limits(line: &str) -> Option<UsageWindows> {
    let event: Value = serde_json::from_str(line).ok()?;
    if event["type"] != "event_msg" || event["payload"]["type"] != "token_count" {
        return None;
    }
    let limits = event["payload"]["rate_limits"].as_object()?;
    let mut windows = UsageWindows::default();
    for slot in ["primary", "secondary"] {
        let Some(window) = limits.get(slot).filter(|w| w.is_object()) else {
            continue;
        };
        let (Some(used_percent), Some(window_minutes)) = (
            window["used_percent"].as_f64(),
            window["window_minutes"].as_u64(),
        ) else {
            continue;
        };
        let parsed = UsageWindow {
            used_percent,
            resets_at: window["resets_at"].as_u64(),
            window_minutes,
        };
        match window_minutes {
            FIVE_HOUR_MINUTES => windows.five_hour = Some(parsed),
            WEEKLY_MINUTES => windows.weekly = Some(parsed),
            _ => {}
        }
    }
    Some(windows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_merge_uses_newest_observation_and_expires_without_claiming_unused_quota() {
        let now = bus_io::now_ms();
        let mut usage = Usage {
            started_at_ms: now - 1000,
            ..Usage::default()
        };
        let observation = |agent, at| claude_statusline::Observation {
            manifest: super::super::callbacks::Manifest {
                agent_id: AgentId(agent),
                provider: Provider::ClaudeCode,
                launch_id: "launch".into(),
            },
            session_id: "session".into(),
            read_at_ms: at,
            windows: UsageWindows {
                five_hour: Some(UsageWindow {
                    used_percent: 20.0,
                    resets_at: Some(now / 1000 + 10),
                    window_minutes: FIVE_HOUR_MINUTES,
                }),
                weekly: Some(UsageWindow {
                    used_percent: 40.0,
                    resets_at: Some(now / 1000 + 10000),
                    window_minutes: WEEKLY_MINUTES,
                }),
            },
        };
        usage.merge_claude(observation(1, now - 2000));
        assert!(
            usage.claude.is_none(),
            "old spool is not a new observation after restart"
        );
        usage.merge_claude(observation(1, now - 900));
        usage.merge_claude(observation(2, now - 500));
        usage.merge_claude(observation(1, now - 800));
        assert_eq!(usage.claude_json(now)["observed_by_agent"], 2);
        assert_eq!(usage.claude_json(now)["five_hour"]["used_percent"], 20.0);
        assert_eq!(usage.claude_json(now + 10000)["five_hour"], Value::Null);
        assert_eq!(
            usage.claude_json(now + 10000)["weekly"]["used_percent"],
            40.0
        );
        assert_eq!(usage.claude_json(now + STALE_AFTER_MS)["status"], "unknown");
        let mut missing = observation(2, now);
        missing.windows = UsageWindows::default();
        usage.merge_claude(missing);
        assert_eq!(usage.claude_json(now)["status"], "unknown");
    }

    fn token_count(limits: Value) -> String {
        json!({"timestamp":"2026-10-05T00:00:00Z","type":"event_msg",
            "payload":{"type":"token_count","info":null,"rate_limits":limits}})
        .to_string()
    }

    #[test]
    fn rollout_uses_newest_rate_limits_and_classifies_windows_by_duration() {
        let dir = std::env::temp_dir().join(format!(
            "bus-usage-{}-{}",
            std::process::id(),
            bus_io::now_ns()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rollout-2026-10-05T00-00-00-abc.jsonl");
        let lines = [
            token_count(
                json!({"primary":{"used_percent":90.0,"window_minutes":300,"resets_at":1}}),
            ),
            // Weekly in primary, five-hour in secondary: slots are not stable.
            token_count(json!({
                "limit_id":"codex",
                "primary":{"used_percent":12.5,"window_minutes":10080,"resets_at":1791788174},
                "secondary":{"used_percent":40.0,"window_minutes":300,"resets_at":1791700000},
            })),
            // Newer lines without rate limits do not hide the last known values.
            token_count(Value::Null),
            json!({"type":"response_item","payload":{"type":"message"}}).to_string(),
        ];
        std::fs::write(&path, format!("{{\"partial\n{}\n", lines.join("\n"))).unwrap();

        let windows = read_codex_rollout(&path).unwrap().unwrap();
        assert_eq!(
            windows.weekly,
            Some(UsageWindow {
                used_percent: 12.5,
                resets_at: Some(1791788174),
                window_minutes: 10080
            })
        );
        assert_eq!(windows.five_hour.unwrap().used_percent, 40.0);

        std::fs::write(
            &path,
            token_count(
                json!({"primary":{"used_percent":3.0,"window_minutes":10080},"secondary":null}),
            ),
        )
        .unwrap();
        let windows = read_codex_rollout(&path).unwrap().unwrap();
        assert_eq!(windows.five_hour, None, "absent window stays unknown");
        assert_eq!(windows.weekly.unwrap().resets_at, None);

        std::fs::write(&path, "not json\n").unwrap();
        assert_eq!(read_codex_rollout(&path).unwrap(), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn only_absolute_codex_rollout_paths_are_read() {
        let accepted =
            json!({"transcript_path":"/Users/me/.codex/sessions/2026/10/05/rollout-1.jsonl"});
        assert!(codex_rollout_path(&accepted).is_some());
        for path in [
            json!("relative/rollout-1.jsonl"),
            json!("/Users/me/.ssh/id_ed25519"),
            json!("/tmp/rollout-1.json"),
            Value::Null,
        ] {
            assert!(
                codex_rollout_path(&json!({"transcript_path":path})).is_none(),
                "{path}"
            );
        }
    }

    #[test]
    fn state_reports_missing_usage_as_unknown_never_as_unused() {
        let usage = Usage::default();
        let value = usage.state_json();
        for provider in ["claude", "codex", "cursor"] {
            assert_eq!(value[provider]["status"], "unknown", "{provider}");
            assert!(value[provider].get("five_hour").is_none(), "{provider}");
        }
        let usage = Usage {
            codex: Some(UsageSnapshot {
                windows: UsageWindows {
                    five_hour: None,
                    weekly: Some(UsageWindow {
                        used_percent: 7.0,
                        resets_at: Some(5),
                        window_minutes: WEEKLY_MINUTES,
                    }),
                },
                read_at_ms: 9,
                observed_by_agent: AgentId(3),
            }),
            ..Usage::default()
        };
        assert_eq!(
            usage.state_json()["codex"],
            json!({"status":"observed","five_hour":null,
                "weekly":{"used_percent":7.0,"resets_at":5,"window_minutes":10080},
                "read_at_ms":9,"observed_by_agent":3})
        );
    }
}
