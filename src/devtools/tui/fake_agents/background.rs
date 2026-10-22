//! The fake Claude's background shells: `fake:shell NAME` lists one on every
//! `Stop`, and `fake:wake N` ends it with a task-notification turn that has
//! no `Stop`, as Claude Code does when it folds the next notification in.
//! Add `fake:wake-stop` to emit that wake turn's final report as well.
use super::directives::{shell_name, wake_seconds};
use super::{submitted, FakeAgent};
use serde_json::{json, Value};
use std::io::Write;
use std::sync::mpsc;
use std::time::{Duration, Instant};

impl FakeAgent {
    /// A Claude `Stop` payload for `reply`: it starts the shell `prompt`
    /// names, schedules its wake, and lists every running shell.
    pub(super) fn stop_payload(&mut self, prompt: &str, reply: &str) -> Value {
        let mut payload = json!({"last_assistant_message": reply, "stop_hook_active": false});
        if self.codex() {
            return payload;
        }
        if let Some(shell) = shell_name(prompt) {
            if let Some(seconds) = wake_seconds(prompt) {
                let at = Instant::now() + Duration::from_secs(seconds.min(600));
                self.wake = Some((at, shell.clone(), prompt.contains("fake:wake-stop")));
            }
            self.shells.push(shell);
        }
        payload["background_tasks"] = self.background_tasks();
        payload["session_crons"] = json!([]);
        payload
    }

    /// Idle input, or a timeout once the scheduled wake `at` is due.
    pub(super) fn receive_or_wake(
        input: &mpsc::Receiver<Vec<u8>>,
        at: Option<Instant>,
    ) -> Result<Vec<u8>, mpsc::RecvTimeoutError> {
        match at {
            Some(at) => input.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => input
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        }
    }

    /// The running shells as Claude Code 2.1.296 lists them on `Stop`.
    pub(super) fn background_tasks(&self) -> Value {
        Value::Array(
            self.shells
                .iter()
                .map(|id| {
                    json!({"id": id, "type": "shell", "status": "running", "description": "Run", "command": "sleep 45"})
                })
                .collect(),
        )
    }

    /// The newest shell ends: Claude wakes a turn with its task notification
    /// (a `UserPromptSubmit`), works a moment, and shows its prompt box again
    /// without a `Stop`, as Claude Code does when the next notification folds
    /// into that turn.
    pub(super) fn wake(&mut self) {
        let Some((_, shell, stop)) = self.wake.take() else {
            return;
        };
        self.shells.retain(|running| *running != shell);
        let turn = self.next_turn();
        let prompt =
            format!("<task-notification><task-id>{shell}</task-id>completed</task-notification>");
        let mut out = std::io::stdout();
        let _ = write!(out, "{}", submitted(&prompt));
        let _ = out.flush();
        self.hook("UserPromptSubmit", Some(&turn), json!({"prompt": prompt}));
        self.spin(1);
        if stop {
            self.hook(
                "Stop",
                Some(&turn),
                json!({
                    "last_assistant_message": format!("fake shell completed: {shell}"),
                    "background_tasks": self.background_tasks(),
                    "session_crons": []
                }),
            );
        }
        self.draw_prompt();
    }
}
