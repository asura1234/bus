//! The fake Codex's rollout: each turn's start, prompt and final answer, in
//! the event shapes Codex 0.163.0 writes. `fake:lose-cwd` plays a worker that
//! removes its own working directory mid-turn: real Codex then runs no
//! further hooks, from that turn's `Stop` on, while its rollout still records
//! every answer (live, 2026-10-20).
use super::FakeAgent;
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;

const LOSE_CWD: &str = "fake:lose-cwd";

impl FakeAgent {
    /// A `rollout-*` file, so Bus reads it as Codex's; it holds no
    /// `token_count`, so Bus reads no usage from it.
    pub(super) fn rollout(&self) -> PathBuf {
        std::env::temp_dir().join(format!("rollout-fake-{}.jsonl", self.session))
    }

    /// Records turn `turn` starting on `prompt`, after its submit hook ran.
    pub(super) fn record_prompt(&mut self, turn: &str, prompt: &str) {
        self.hooks_lost |= self.codex() && prompt.contains(LOSE_CWD);
        self.record(json!({"type":"task_started","turn_id":turn}));
        self.record_steering(turn, prompt);
    }

    /// Records input consumed by an already-running turn, without inventing
    /// another task-start event for the same turn.
    pub(super) fn record_steering(&self, turn: &str, prompt: &str) {
        self.record(json!({"type":"item_completed","turn_id":turn,
            "item":{"type":"UserMessage","content":[{"type":"text","text":prompt}]}}));
    }

    /// Records turn `turn` completing with `answer`, before its `Stop`.
    pub(super) fn record_answer(&self, turn: &str, answer: &str) {
        self.record(json!({"type":"task_complete","turn_id":turn,"last_agent_message":answer}));
    }

    fn record(&self, payload: Value) {
        if !self.codex() {
            return;
        }
        let timestamp = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();
        let line = json!({"timestamp":timestamp,"type":"event_msg","payload":payload});
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.rollout())
        {
            let _ = writeln!(file, "{line}");
        }
    }
}
