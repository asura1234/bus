//! `fake:question`: a Codex turn that queues an async question and keeps
//! working, drawn as Codex 0.162 draws it (tests/fixtures/codex-question):
//! on the alternate screen, with a startup warning beside the shortcuts hint.
//! Shift+Left opens the question, Up/Down move, Enter answers.
use super::{drain_input, FakeAgent, Input};
use std::io::Write;
use std::time::{Duration, Instant};

const OPTIONS: [&str; 3] = ["Alpha", "Beta", "Other"];

const TEXT_QUESTION: &str = "• Working (0s • esc to interrupt)\r\n\r\n• Queued follow-up inputs\r\n\r\n  请提供该 bug 的 record id 或行链接\r\n\r\n  Type your answer\r\n\r\n  enter submit   ctrl+] skip   shift+→ main prompt\r\n";

pub(super) fn requested(provider: &str, prompt: &str) -> bool {
    provider == "codex" && matches!(prompt.trim(), "fake:question" | "fake:question-text")
}

fn collapsed() -> String {
    concat!(
        "• Working (0s • esc to interrupt)\r\n\r\n",
        "• Queued follow-up inputs\r\n",
        "  ? 1 question\r\n",
        "    ⇧← to answer\r\n\r\n\r\n",
        "› Ask Codex to do anything\r\n\r\n",
        "  gpt-fake · fake codex\r\n",
        "  ? for shortcuts                                        ⚠ 1 warning · f2 to view\r\n",
    )
    .into()
}

fn expanded(selected: usize) -> String {
    let options: String = OPTIONS
        .iter()
        .enumerate()
        .map(|(index, label)| {
            let marker = if index == selected { "›" } else { " " };
            format!("  {marker} {}. {label}\r\n", index + 1)
        })
        .collect();
    format!(
        "• Working (0s • esc to interrupt)\r\n\r\n• Queued follow-up inputs\r\n\r\n\r\n  Which design should the worker use?\r\n\r\n{options}\r\n  enter submit   ⌃] skip   ⇧→ main prompt\r\n"
    )
}

enum Key {
    Open,
    Up,
    Down,
    Enter,
    Interrupt,
}

/// Keys this fixture reacts to; every other byte is dropped.
fn keys(pending: &mut Vec<u8>) -> Vec<Key> {
    let mut found = Vec::new();
    let mut i = 0;
    while i < pending.len() {
        let rest = &pending[i..];
        let matched = [
            (&b"\x1b[1;2D"[..], Key::Open),
            (b"\x1b[A", Key::Up),
            (b"\x1bOA", Key::Up),
            (b"\x1b[B", Key::Down),
            (b"\x1bOB", Key::Down),
            (b"\x1b[13u", Key::Enter),
            (b"\r", Key::Enter),
            (b"\x03", Key::Interrupt),
        ]
        .into_iter()
        .find(|(sequence, _)| rest.starts_with(sequence));
        match matched {
            Some((sequence, key)) => {
                found.push(key);
                i += sequence.len();
            }
            // Wait for the rest of a sequence split across reads.
            None if rest[0] == 0x1b && rest.len() < 6 => break,
            None => i += 1,
        }
    }
    pending.drain(..i);
    found
}

impl FakeAgent {
    /// Holds the turn on the queued question until it is answered (bounded at
    /// 600 seconds), then leaves the alternate screen and lets the turn reply.
    pub(super) fn queued_question(&mut self, free_text: bool) -> Result<(), i32> {
        let mut out = std::io::stdout();
        let draw = |out: &mut std::io::Stdout, screen: &str| {
            let _ = write!(out, "\x1b[H\x1b[J{screen}");
            let _ = out.flush();
        };
        let _ = write!(out, "\x1b[?1049h");
        let result = if free_text {
            draw(&mut out, TEXT_QUESTION);
            self.hold_text_question()
        } else {
            draw(&mut out, &collapsed());
            self.hold_question(&mut out, draw)
        };
        let _ = write!(out, "\x1b[?1049l");
        let _ = out.flush();
        result
    }

    fn hold_text_question(&mut self) -> Result<(), i32> {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(600) {
            if !self.receive_pending()? {
                continue;
            }
            for input in drain_input(&mut self.pending) {
                match input {
                    Input::Enter => return Ok(()),
                    Input::Interrupt => return Err(0),
                    _ => {}
                }
            }
        }
        Err(1)
    }

    fn hold_question(
        &mut self,
        out: &mut std::io::Stdout,
        draw: impl Fn(&mut std::io::Stdout, &str),
    ) -> Result<(), i32> {
        let started = Instant::now();
        let mut selected: Option<usize> = None;
        while started.elapsed() < Duration::from_secs(600) {
            if !self.receive_pending()? {
                continue;
            }
            for key in keys(&mut self.pending) {
                match (key, selected) {
                    (Key::Interrupt, _) => return Err(0),
                    (Key::Open, None) => selected = Some(0),
                    (Key::Up, Some(index)) => selected = Some(index.saturating_sub(1)),
                    (Key::Down, Some(index)) => selected = Some((index + 1).min(OPTIONS.len() - 1)),
                    (Key::Enter, Some(index)) if index + 1 < OPTIONS.len() => return Ok(()),
                    _ => continue,
                }
                if let Some(index) = selected {
                    draw(out, &expanded(index));
                }
            }
        }
        Err(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn question_keys_wait_for_split_sequences_and_drop_the_rest() {
        let mut pending = b"x\x1b[1;".to_vec();
        assert!(keys(&mut pending).is_empty());
        assert_eq!(pending, b"\x1b[1;");
        pending.extend(b"2D\x1b[B\r");
        let found = keys(&mut pending);
        assert!(matches!(
            found.as_slice(),
            [Key::Open, Key::Down, Key::Enter]
        ));
        assert!(pending.is_empty());
    }
}
