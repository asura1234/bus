//! Pin the visible draft and model only Bus's queued line-editing keys.
use super::{input, AgentKind, InputObservation};
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
    pub(super) fn diagnose_with_policy(
        &self,
        screen: &str,
        phase: &'static str,
        cursor: Option<(u16, u16)>,
        policy: Option<super::diagnostics::Policy>,
    ) {
        let body = input(self.kind, screen);
        let history = self
            .state
            .lock()
            .ok()
            .map(|owned| super::diagnostics::History {
                tracked_body: owned.body.is_some(),
                lost: owned.lost,
                previous_body_bytes: owned.body.as_ref().map_or(0, String::len),
                prior_body_match: owned.body.is_some() && owned.body == body,
                remainder_match: body
                    .as_ref()
                    .is_some_and(|body| owned.remainders.contains(body)),
                remainder_count: owned.remainders.len(),
                modeled_text_bytes: owned.text.len(),
                modeled_cursor: owned.cursor,
            });
        super::diagnostics::refusal(
            self.kind,
            screen,
            &self.expected,
            phase,
            cursor,
            history,
            policy,
        );
    }

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
    fn diagnose_refusal(&self, screen: &str, phase: &'static str, cursor: Option<(u16, u16)>) {
        self.diagnose_with_policy(screen, phase, cursor, None);
    }

    fn observe(&self, screen: &str) -> InputObservation {
        self.observe_with_cursor(screen, None)
    }

    fn observe_with_cursor(&self, screen: &str, cursor: Option<(u16, u16)>) -> InputObservation {
        let observation = super::observe_with_cursor(self.kind, screen, &self.expected, cursor);
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
                let body = super::input_with_cursor(self.kind, screen, cursor);
                if owned.body.is_none() {
                    if observation == InputObservation::Pending {
                        owned.body = body;
                        owned.progress = owned.progress.wrapping_add(1);
                    }
                    return observation;
                }
                if accept_owned_change(&mut owned, body) {
                    InputObservation::Pending
                } else {
                    owned.lost = true;
                    InputObservation::Other
                }
            }
            // Grok refuses every body that is not the full paste. A line kill
            // therefore looks unavailable even when it is the remainder of the
            // draft Bus just typed. Keep clearing only that owned remainder.
            // A developer edit is not one of those remainders: mark it lost
            // and leave it. Do not treat a prefix as owned before the full
            // paste was recognized, or a developer prefix could be submitted.
            InputObservation::Unavailable if self.kind == AgentKind::Grok => {
                if owned.lost || owned.body.is_none() {
                    return InputObservation::Unavailable;
                }
                let body = input(self.kind, screen);
                if accept_owned_change(&mut owned, body) {
                    InputObservation::Pending
                } else {
                    owned.lost = true;
                    InputObservation::Unavailable
                }
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
        let previous = owned.text.clone();
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
        // A scrolled 1.0.70 draft proves ownership from a visible suffix, so
        // the same cut is also authorized when that suffix still overlaps the
        // shortened paste.
        let body = owned.body.clone();
        if let Some(remaining) = body.as_ref().and_then(|body| {
            remaining_prefix(body, &owned.text)
                .or_else(|| remaining_attested_suffix(body, &previous, &owned.text))
        }) {
            if Some(&remaining) != owned.body.as_ref()
                && owned.remainders.back() != Some(&remaining)
            {
                owned.remainders.push_back(remaining);
            }
        }
    }
}

fn accept_owned_change(owned: &mut Owned, body: Option<String>) -> bool {
    if body == owned.body {
        return true;
    }
    let Some(at) = owned
        .remainders
        .iter()
        .position(|remaining| Some(remaining) == body.as_ref())
    else {
        return false;
    };
    owned.remainders.drain(..=at);
    owned.body = body;
    owned.progress = owned.progress.wrapping_add(1);
    true
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

/// The visible body is a suffix of the paste from before this key, and the
/// shortened paste is still a prefix of that text. Authorize the prefix of
/// the original visible body that the shortened paste still covers.
fn remaining_attested_suffix(body: &str, previous: &str, next: &str) -> Option<String> {
    let body_norm = normalize_visible(body);
    let old_norm = normalize(previous);
    let new_norm = normalize(next);
    let old_count = old_norm.chars().count();
    let body_count = body_norm.chars().count();
    let new_count = new_norm.chars().count();
    if body_count == 0
        || old_count < body_count
        || !old_norm.ends_with(&body_norm)
        || !old_norm.starts_with(&new_norm)
    {
        return None;
    }
    let hidden = old_count - body_count;
    if new_count < hidden {
        return None;
    }
    let keep = new_count - hidden;
    // Deleting only off-screen text leaves the visible suffix unchanged.
    if keep == 0 || keep >= body_count {
        return None;
    }
    let mut seen = 0;
    for (at, character) in body.char_indices() {
        if character.is_whitespace() || is_scroll_rail(character) {
            continue;
        }
        seen += 1;
        if seen == keep {
            let remaining = body[..at + character.len_utf8()].trim_end().to_owned();
            return (remaining != body).then_some(remaining);
        }
    }
    None
}

fn normalize_visible(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_whitespace() && !is_scroll_rail(*character))
        .collect()
}

fn is_scroll_rail(character: char) -> bool {
    ('▁'..='█').contains(&character)
}
