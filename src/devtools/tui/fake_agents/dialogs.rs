//! Inert native Codex dialog fixture, independent of permission handlers.
use super::{drain_input, FakeAgent, Input};
use std::io::Write;
use std::time::{Duration, Instant};

pub(super) fn redraw_requested(prompt: &str) -> bool {
    matches!(
        prompt.trim(),
        "fake:dialog-settle redraw" | "fake:dialog-null"
    )
}

const SCREEN: &str = "Would you like to run the following command?\r\n\r\n$ CARGO_TARGET_DIR=./target cargo test dialog_settle_probe --locked\r\n\r\n› 1. Yes, proceed (y)\r\n  2. No, and tell Codex what to do differently (esc)\r\n\r\npress enter to confirm or esc to cancel\r\n";

impl FakeAgent {
    /// Hold until native input, then deliberately tear post-answer snapshots.
    /// The displayed shell command is never executed.
    pub(super) fn settlement_dialog(&mut self, prompt: &str) -> Result<(), i32> {
        if self.live_turn() {
            self.end_turn(&[]);
        }
        let mut out = std::io::stdout();
        let soft_wrapped = prompt.trim() == "fake:dialog-null";
        let screen = if soft_wrapped {
            include_str!(
                "../../../server/api/agents/tests/fixtures/codex_soft_wrapped_permission.txt"
            )
            .replace('\n', "\r\n")
        } else {
            SCREEN.to_owned()
        };
        let _ = write!(out, "{screen}");
        let _ = out.flush();
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(600) {
            let bytes = match self
                .input
                .as_ref()
                .map(|rx| rx.recv_timeout(Duration::from_millis(100)))
            {
                Some(Ok(bytes)) => bytes,
                Some(Err(std::sync::mpsc::RecvTimeoutError::Timeout)) => continue,
                _ => return Err(0),
            };
            self.pending.extend(bytes);
            for input in drain_input(&mut self.pending) {
                match input {
                    Input::Enter => {
                        // Let native choose acknowledge its one write first.
                        std::thread::sleep(Duration::from_millis(50));
                        let rows = screen.matches("\r\n").count();
                        let clear = if soft_wrapped {
                            "\x1b[2J\x1b[H".to_owned()
                        } else {
                            format!("\x1b[{rows}A\r\x1b[J")
                        };
                        let _ = write!(out, "\x1b[?2026h{clear}• You approved codex to run the fixture this time\r\n");
                        let _ = out.flush();
                        std::thread::sleep(Duration::from_millis(350));
                        let _ = write!(out, "\x1b[?2026l");
                        let _ = out.flush();
                        // Give submit's normal end_turn its own live frame to
                        // remove, preserving the acknowledgement above it.
                        if self.live_turn() {
                            self.begin_turn();
                        }
                        return Ok(());
                    }
                    Input::Interrupt => return Err(0),
                    _ => {}
                }
            }
        }
        Err(1)
    }
}
