//! Live simulated work consumes steering with explicit turn ownership.
use super::{drain_input, FakeAgent, Turn, Typed};
use serde_json::json;
use std::sync::mpsc;

impl FakeAgent {
    /// Keeps a live turn working until `seconds` after it began, reading
    /// input, so Bus can type a steering message into the running turn. Each
    /// prompt entered meanwhile joins the turn, whose one reply answers it
    /// too. Returns those prompts and whether the stop key ended the turn
    /// early, or the exit code when the provider was quit.
    pub(super) fn work(&mut self, seconds: u64, turn: &str) -> Result<Turn, i32> {
        let mut steered = Vec::new();
        while let Some((started, frame)) = self.working {
            if started.elapsed().as_secs() >= seconds {
                break;
            }
            let tick = std::time::Duration::from_millis(120);
            let bytes = match self.input.as_ref().map(|input| input.recv_timeout(tick)) {
                Some(Ok(bytes)) => bytes,
                Some(Err(mpsc::RecvTimeoutError::Timeout)) => Vec::new(),
                _ => {
                    std::thread::sleep(tick);
                    Vec::new()
                }
            };
            self.pending.extend_from_slice(&bytes);
            for input in drain_input(&mut self.pending) {
                match self.apply(input) {
                    Some(Typed::Submit(prompt)) => {
                        // This fake actually consumes the input in this turn.
                        // Publish that ownership, rather than asking Bus to
                        // infer it from a working/idle screen or elapsed time.
                        let event = if self.cursor() {
                            "beforeSubmitPrompt"
                        } else {
                            "UserPromptSubmit"
                        };
                        self.hook(event, Some(turn), json!({"prompt":prompt}));
                        self.record_steering(turn, &prompt);
                        steered.push(prompt);
                    }
                    Some(Typed::Stop) => return Ok(Turn::Stopped(steered)),
                    Some(Typed::Exit) => return Err(0),
                    None => {}
                }
            }
            self.working = Some((started, frame + 1));
            self.redraw();
        }
        Ok(Turn::Done(steered))
    }
}
