//! Preserve complete native objects for Claude; exact editor text elsewhere.
use super::{input, ownership::OwnedComposer, AgentKind, InputObservation};
use crate::terminal::runtime::{DraftPreservation, EditorSubmission, SubmissionInspector};
use bytes::Bytes;
use std::{path::PathBuf, sync::Mutex, time::Duration};

pub(super) struct PreservingComposer {
    kind: AgentKind,
    owned: OwnedComposer,
    expected: String,
    directory: PathBuf,
    keys: Vec<Bytes>,
    steer: bool,
    native_draft: Mutex<Option<NativeDraft>>,
}

enum NativeDraft {
    Captured(String),
    ExistingStash,
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
            owned: OwnedComposer::new(kind, expected.clone()),
            expected,
            directory,
            keys,
            steer: false,
            native_draft: Mutex::new(None),
        }
    }

    pub(super) fn with_steering(mut self, steer: bool) -> Self {
        self.steer = steer;
        self
    }
}

impl PreservingComposer {
    fn choose(
        &self,
        screen: &str,
        cursor: Option<(u16, u16)>,
        allow_empty: bool,
        pristine: bool,
    ) -> Option<DraftPreservation> {
        let body = super::input_since_launch(self.kind, screen, cursor, pristine)?;
        if (body.is_empty() && !allow_empty) || self.keys.len() != 3 {
            return None;
        }
        match self.kind {
            AgentKind::Claude => {
                // Claude's stash has one slot. Preserve any stash the developer
                // already owns; a second stash would silently replace it.
                if native_stash_present(screen) {
                    return None;
                }
                *self.native_draft.lock().ok()? = Some(NativeDraft::Captured(body.clone()));
                Some(DraftPreservation::Native {
                    key: self.keys[0].clone(),
                    directory: self.directory.clone(),
                    visible_draft: body,
                })
            }
            AgentKind::Codex | AgentKind::Cursor | AgentKind::Kimi if !opaque(&body) => {
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

pub(super) fn opaque(body: &str) -> bool {
    ["[Image #", "[Pasted text #", "[Pasted Content "]
        .iter()
        .any(|marker| body.contains(marker))
}

#[cfg(test)]
#[path = "../../tests/draft_policy_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../tests/claude_rich_draft_test.rs"]
mod claude_rich_draft_tests;

impl SubmissionInspector for PreservingComposer {
    fn retry_owned_enter(&self) -> bool {
        self.owned.retry_owned_enter()
    }
    fn native_draft_preserved(&self) -> bool {
        self.native_draft
            .lock()
            .map_or(true, |draft| draft.is_some())
    }
    fn confirm_consumption(&self) -> bool {
        self.owned.confirm_consumption()
    }
    fn recoverable_input(&self, screen: &str, cursor: Option<(u16, u16)>) -> bool {
        self.owned.recoverable_input(screen, cursor)
    }
    fn owned_popup_escape(&self, screen: &str) -> Option<bytes::Bytes> {
        self.owned.owned_popup_escape(screen)
    }
    fn submission_available(&self, screen: &str) -> bool {
        super::submission_available(self.kind, screen)
    }
    fn paste_ready(&self, screen: &str) -> bool {
        if !super::paste_ready(self.kind, screen, &self.expected) {
            return false;
        }
        if self.kind != AgentKind::Claude {
            return true;
        }
        let Ok(saved) = self.native_draft.lock() else {
            return false;
        };
        let wait_for_input = match saved.as_ref() {
            Some(NativeDraft::Captured(body)) => opaque(body),
            Some(NativeDraft::ExistingStash) => true,
            None => false,
        };
        drop(saved);
        // A prior timeout may retain the native stash after its late paste is
        // withdrawn. Both that retry and a fresh rich-draft swap need the new
        // editable body, even for paths outside the attachment policy.
        !wait_for_input || input(self.kind, screen).is_some_and(|body| !body.is_empty())
    }
    fn draft_ready(&self, screen: &str, cursor: Option<(u16, u16)>, pristine: bool) -> bool {
        super::draft_ready(self.kind, screen, cursor, pristine)
    }
    fn always_use_editor(&self) -> bool {
        self.kind == AgentKind::Kimi
    }
    fn diagnose_refusal(&self, screen: &str, phase: &'static str, cursor: Option<(u16, u16)>) {
        self.owned.diagnose_with_policy(
            screen,
            phase,
            cursor,
            Some(super::diagnostics::Policy {
                keys_available: self.keys.len() == 3,
                native_stash: native_stash_present(screen),
                invisible_content: invisible_content(self.kind, screen, cursor),
                opaque_draft: input(self.kind, screen).as_deref().is_some_and(opaque),
            }),
        );
    }

    fn observe(&self, screen: &str) -> InputObservation {
        self.observe_with_launch(screen, None, false)
    }
    fn observe_with_cursor(&self, screen: &str, cursor: Option<(u16, u16)>) -> InputObservation {
        self.observe_with_launch(screen, cursor, false)
    }
    fn observe_with_launch(
        &self,
        screen: &str,
        cursor: Option<(u16, u16)>,
        pristine: bool,
    ) -> InputObservation {
        self.owned.observe_with_launch(screen, cursor, pristine)
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

    fn editor_submission(&self) -> Option<EditorSubmission> {
        (matches!(
            self.kind,
            AgentKind::Kimi | AgentKind::Codex | AgentKind::Cursor
        ) && self.keys.len() == 3)
            .then(|| EditorSubmission {
                text: self.expected.replace("\r\n", "\n").replace('\r', "\n"),
                directory: self.directory.clone(),
                key: self.keys[1].clone(),
                backspace: self.keys[2].clone(),
            })
    }

    fn editor_submit_key(&self, screen: &str, default: &Bytes) -> Bytes {
        if self.kind == AgentKind::Kimi
            && self.steer
            && self.keys.len() == 3
            && super::super::kimi::working(screen)
        {
            self.keys[0].clone()
        } else {
            default.clone()
        }
    }

    fn draft_preservation(&self, screen: &str) -> Option<DraftPreservation> {
        self.draft_preservation_since_launch(screen, None, false)
    }
    fn draft_preservation_with_cursor(
        &self,
        screen: &str,
        cursor: Option<(u16, u16)>,
    ) -> Option<DraftPreservation> {
        self.draft_preservation_since_launch(screen, cursor, false)
    }
    fn draft_preservation_since_launch(
        &self,
        screen: &str,
        cursor: Option<(u16, u16)>,
        pristine: bool,
    ) -> Option<DraftPreservation> {
        let invisible = invisible_content(self.kind, screen, cursor);
        if self.kind == AgentKind::Claude
            && !invisible
            && native_stash_present(screen)
            && super::input_since_launch(self.kind, screen, cursor, pristine)
                .is_some_and(|body| body.is_empty())
        {
            *self.native_draft.lock().ok()? = Some(NativeDraft::ExistingStash);
            return None;
        }
        self.choose(screen, cursor, invisible, pristine)
            .or_else(|| invisible.then_some(DraftPreservation::Hold))
    }
    fn native_stash_visible(&self, screen: &str) -> bool {
        native_stash_present(screen)
    }

    fn native_draft_restored(&self, screen: &str) -> bool {
        if native_stash_present(screen) {
            return false;
        }
        let Ok(saved) = self.native_draft.lock() else {
            return false;
        };
        match saved.as_ref() {
            Some(NativeDraft::Captured(draft)) => input(self.kind, screen).as_ref() == Some(draft),
            Some(NativeDraft::ExistingStash) => input(self.kind, screen).is_some(),
            None => false,
        }
    }

    fn restoration_marker_visible(&self, screen: &str, marker: &str) -> bool {
        input(self.kind, screen).is_some_and(|body| {
            // Codex soft-wraps the native editor receipt after its PID
            // separator. Match the exact terminal token across rendered rows.
            if matches!(self.kind, AgentKind::Kimi | AgentKind::Codex) {
                body.chars()
                    .filter(|c| !c.is_whitespace())
                    .collect::<String>()
                    .ends_with(marker)
            } else {
                body.ends_with(marker)
            }
        })
    }
}

pub(super) fn native_stash_present(screen: &str) -> bool {
    let rows: Vec<String> = screen
        .lines()
        .map(|line| {
            super::styled_chars(line)
                .iter()
                .map(|c| c.character)
                .collect()
        })
        .collect();
    let Some(top) = rows
        .windows(2)
        .rposition(|pair| pair[0].trim().starts_with('─') && pair[1].trim().starts_with('❯'))
    else {
        return false;
    };
    let below = rows[top + 2..]
        .iter()
        .position(|row| row.trim().starts_with('─'))
        .and_then(|end| rows.get(top + 3 + end));
    rows.get(top.wrapping_sub(1))
        .into_iter()
        .chain(below)
        .any(|row| row.trim_end().ends_with("› stashed"))
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
    if !lines[row][offset..]
        .trim_start_matches(markers)
        .trim()
        .is_empty()
    {
        return false;
    }
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
