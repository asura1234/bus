//! Pin the visible draft and model only Bus's queued line-editing keys.
use super::{input, observe, AgentKind, InputObservation};
use crate::terminal::runtime::SubmissionInspector;
use std::{collections::VecDeque, sync::Mutex};

pub(super) struct OwnedComposer {
    kind: AgentKind,
    expected: String,
    state: Mutex<Owned>,
}

struct Owned {
    body: Option<String>,
    text: String,
    cursor: usize,
    remainders: VecDeque<String>,
    lost: bool,
    progress: u64,
}

fn normalize(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

impl OwnedComposer {
    pub(super) fn new(kind: AgentKind, expected: String) -> Self {
        let text = expected.replace("\r\n", "\n").replace('\r', "\n");
        Self {
            kind,
            expected,
            state: Mutex::new(Owned {
                cursor: text.len(),
                text,
                body: None,
                remainders: VecDeque::new(),
                lost: false,
                progress: 0,
            }),
        }
    }
}

impl SubmissionInspector for OwnedComposer {
    fn observe(&self, screen: &str) -> InputObservation {
        let observation = observe(self.kind, screen, &self.expected);
        let Ok(mut owned) = self.state.lock() else {
            return InputObservation::Unavailable;
        };
        match observation {
            InputObservation::Empty => {
                owned.body = None;
                owned.text = self.expected.replace("\r\n", "\n").replace('\r', "\n");
                owned.cursor = owned.text.len();
                owned.remainders.clear();
                owned.lost = false;
                InputObservation::Empty
            }
            InputObservation::Pending | InputObservation::Other => {
                if owned.lost {
                    return InputObservation::Other;
                }
                let body = input(self.kind, screen);
                if owned.body.is_none() {
                    if observation == InputObservation::Pending {
                        owned.body = body;
                        owned.progress = owned.progress.wrapping_add(1);
                    }
                    return observation;
                }
                if body == owned.body {
                    return InputObservation::Pending;
                }
                if let Some(at) = owned
                    .remainders
                    .iter()
                    .position(|remaining| Some(remaining) == body.as_ref())
                {
                    owned.remainders.drain(..=at);
                    owned.body = body;
                    owned.progress = owned.progress.wrapping_add(1);
                    return InputObservation::Pending;
                }
                owned.lost = true;
                InputObservation::Other
            }
            other => other,
        }
    }

    fn cleanup_progress(&self) -> u64 {
        self.state.lock().map_or(0, |owned| owned.progress)
    }

    fn clearing(&self, index: usize) {
        let Ok(mut owned) = self.state.lock() else {
            return;
        };
        if owned.body.is_none() || owned.lost {
            return;
        }
        let cursor = owned.cursor;
        let start = owned.text[..cursor].rfind('\n').map_or(0, |at| at + 1);
        let end = owned.text[cursor..]
            .find('\n')
            .map_or(owned.text.len(), |at| cursor + at);
        match index {
            0 => owned.cursor = start, // Ctrl+A: beginning of logical line.
            1 => {
                // Ctrl+K: remaining line, or its following newline.
                let end = if cursor == end && end < owned.text.len() {
                    end + 1
                } else {
                    end
                };
                owned.text.replace_range(cursor..end, "");
            }
            2 => {
                // Ctrl+U: preceding line text, or its preceding newline.
                let start = if cursor == start {
                    start.saturating_sub(1)
                } else {
                    start
                };
                owned.text.replace_range(start..cursor, "");
                owned.cursor = start;
            }
            _ => return,
        }
        // Preserve the exact visible spacing. Only the prefix cut at the
        // modelled logical-line boundary is authorized, never arbitrary edits.
        if let Some(remaining) = owned
            .body
            .as_ref()
            .and_then(|body| remaining_prefix(body, &owned.text))
        {
            if Some(&remaining) != owned.body.as_ref()
                && owned.remainders.back() != Some(&remaining)
            {
                owned.remainders.push_back(remaining);
            }
        }
    }
}

fn remaining_prefix(body: &str, text: &str) -> Option<String> {
    let normalized = normalize(text);
    let mut expected = normalized.chars();
    let mut end = 0;
    for (at, character) in body.char_indices().filter(|(_, c)| !c.is_whitespace()) {
        match expected.next() {
            Some(next) if next == character => end = at + character.len_utf8(),
            None => break,
            _ => return None,
        }
    }
    expected
        .next()
        .is_none()
        .then(|| body[..end].trim_end().to_owned())
}
