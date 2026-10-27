//! Preserve complete native objects for Claude; exact editor text elsewhere.
use super::{input, ownership::OwnedComposer, AgentKind, InputObservation};
use crate::terminal::runtime::{DraftPreservation, SubmissionInspector};
use bytes::Bytes;
use std::{path::PathBuf, sync::Mutex, time::Duration};

pub(super) struct PreservingComposer {
    kind: AgentKind,
    owned: OwnedComposer,
    directory: PathBuf,
    keys: Vec<Bytes>,
    native_draft: Mutex<Option<String>>,
}

impl PreservingComposer {
    pub(super) fn new(
        kind: AgentKind,
        expected: String,
        directory: PathBuf,
        keys: Vec<Bytes>,
    ) -> Self {
        Self {
            kind,
            owned: OwnedComposer::new(kind, expected),
            directory,
            keys,
            native_draft: Mutex::new(None),
        }
    }
}

impl PreservingComposer {
    fn choose(&self, screen: &str, allow_empty: bool) -> Option<DraftPreservation> {
        let body = input(self.kind, screen)?;
        if (body.is_empty() && !allow_empty) || self.keys.len() != 3 {
            return None;
        }
        match self.kind {
            AgentKind::Claude => {
                // Claude's stash has one slot. Preserve any stash the developer
                // already owns; a second stash would silently replace it.
                if screen.contains(" stashed") {
                    return None;
                }
                *self.native_draft.lock().ok()? = Some(body.clone());
                Some(DraftPreservation::Native {
                    key: self.keys[0].clone(),
                    directory: self.directory.clone(),
                    visible_draft: body,
                })
            }
            AgentKind::Codex | AgentKind::Cursor if !opaque(&body) => {
                Some(DraftPreservation::Editor {
                    key: self.keys[1].clone(),
                    backspace: self.keys[2].clone(),
                    directory: self.directory.clone(),
                })
            }
            _ => None,
        }
    }
}

fn opaque(body: &str) -> bool {
    ["[Image #", "[Pasted text #", "[Pasted Content "]
        .iter()
        .any(|marker| body.contains(marker))
}

#[cfg(test)]
#[path = "../../tests/draft_policy_test.rs"]
mod tests;

impl SubmissionInspector for PreservingComposer {
    fn observe(&self, screen: &str) -> InputObservation {
        self.owned.observe(screen)
    }
    fn clearing(&self, index: usize) {
        self.owned.clearing(index);
    }
    fn cleanup_progress(&self) -> u64 {
        self.owned.cleanup_progress()
    }
    fn preservation_budget(&self) -> Duration {
        crate::protocol::api::submission::DRAFT_TRANSFER_BUDGET
    }

    fn draft_preservation(&self, screen: &str) -> Option<DraftPreservation> {
        self.choose(screen, false)
    }
    fn draft_preservation_with_cursor(
        &self,
        screen: &str,
        cursor: Option<(u16, u16)>,
    ) -> Option<DraftPreservation> {
        if self.kind == AgentKind::Claude && screen.contains(" stashed") {
            return Some(DraftPreservation::Hold);
        }
        self.choose(screen, invisible_content(self.kind, screen, cursor))
    }
    fn native_stash_visible(&self, screen: &str) -> bool {
        screen.contains(" stashed")
    }

    fn native_draft_restored(&self, screen: &str) -> bool {
        self.native_draft
            .lock()
            .ok()
            .and_then(|draft| draft.clone())
            .is_some_and(|draft| {
                input(self.kind, screen).as_ref() == Some(&draft) && !screen.contains(" stashed")
            })
    }

    fn restoration_marker_visible(&self, screen: &str, marker: &str) -> bool {
        input(self.kind, screen).is_some_and(|body| body.ends_with(marker))
    }
}

fn invisible_content(kind: AgentKind, screen: &str, cursor: Option<(u16, u16)>) -> bool {
    use unicode_width::UnicodeWidthChar;
    let Some((x, y)) = cursor else {
        return false;
    };
    let markers: &[char] = match kind {
        AgentKind::Claude => &['❯'],
        AgentKind::Codex => &['›'],
        AgentKind::Cursor => &['>', '→'],
        _ => return false,
    };
    let lines: Vec<String> = screen
        .lines()
        .map(|line| {
            super::styled_chars(line)
                .iter()
                .map(|c| c.character)
                .collect()
        })
        .collect();
    let Some((row, offset)) = lines.iter().enumerate().rev().find_map(|(row, line)| {
        line.char_indices()
            .find(|(_, c)| markers.contains(c))
            .map(|(offset, _)| (row, offset))
    }) else {
        return false;
    };
    let end = lines[row + 1..]
        .iter()
        .position(|line| {
            let line = line.trim();
            line.starts_with(['─', '╰', '└', '▀'])
                || line.starts_with("? for")
                || line.starts_with("/ commands")
                || kind == AgentKind::Codex
                    && (line.starts_with("GPT-") || line.starts_with("gpt-"))
        })
        .map_or(lines.len(), |end| row + 1 + end);
    let start = lines[row][..offset]
        .chars()
        .map(|c| c.width().unwrap_or(0))
        .sum::<usize>()
        + 2;
    usize::from(y) < end
        && (usize::from(y) > row || usize::from(y) == row && usize::from(x) > start)
}

pub(super) fn default_claude_stash_binding() -> bool {
    let root = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|home| home.join(".claude")));
    let Some(root) = root else {
        return false;
    };
    matches!(std::fs::read(root.join("keybindings.json")), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
}
