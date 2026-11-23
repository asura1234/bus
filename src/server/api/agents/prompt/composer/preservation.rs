//! Preserve complete native objects for Claude; exact editor text elsewhere.
use super::{input, ownership::OwnedComposer, AgentKind, InputObservation};
use crate::terminal::runtime::{DraftPreservation, EditorSubmission, SubmissionInspector};
use bytes::Bytes;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

pub(super) struct PreservingComposer {
    kind: AgentKind,
    owned: OwnedComposer,
    aborted: Option<Mutex<AbortedMatcher>>,
    discarded_aborted_payload: Arc<AtomicBool>,
    expected: String,
    directory: PathBuf,
    keys: Vec<Bytes>,
    steer: bool,
    bracketed_paste: bool,
    native_draft: Mutex<Option<NativeDraft>>,
}

struct AbortedMatcher {
    payload: String,
    normalized: String,
    collapsed: Option<String>,
    started: bool,
    last_len: usize,
    progress: u64,
}

impl AbortedMatcher {
    fn new(payload: String) -> Self {
        let normalized = payload.chars().filter(|c| !c.is_whitespace()).collect();
        Self {
            payload,
            normalized,
            collapsed: None,
            started: false,
            last_len: usize::MAX,
            progress: 0,
        }
    }

    fn observe(
        &mut self,
        kind: AgentKind,
        screen: &str,
        cursor: Option<(u16, u16)>,
        pristine: bool,
        unedited_stop: bool,
    ) -> InputObservation {
        let Some(body) = super::input_since_launch(kind, screen, cursor, pristine) else {
            return InputObservation::Unavailable;
        };
        if body.is_empty() {
            return if self.started {
                InputObservation::Empty
            } else {
                InputObservation::Other
            };
        }
        if !self.started {
            let collapsed = kind == AgentKind::Claude && super::pasted_text_count(&body).is_some();
            if collapsed && !unedited_stop {
                return InputObservation::Other;
            }
            if super::observe_since_launch(kind, screen, &self.payload, cursor, pristine)
                == InputObservation::Pending
            {
                self.started = true;
                self.last_len = body.len();
                self.progress = self.progress.wrapping_add(1);
                if collapsed {
                    self.collapsed = Some(body);
                }
                return InputObservation::Pending;
            }
            return InputObservation::Other;
        }
        // Claude renders a collapsed paste as one atom. A cursor move leaves
        // its label unchanged, and a line kill removes it in one edit.
        if let Some(collapsed) = &self.collapsed {
            return if unedited_stop && *collapsed == body {
                InputObservation::Pending
            } else {
                InputObservation::Other
            };
        }
        let norm: String = body.chars().filter(|c| !c.is_whitespace()).collect();
        let mut hay = self.normalized.chars();
        if body.len() <= self.last_len
            && norm.len() <= self.normalized.len()
            && norm.chars().all(|c| hay.any(|h| h == c))
        {
            if body.len() < self.last_len {
                self.last_len = body.len();
                self.progress = self.progress.wrapping_add(1);
            }
            InputObservation::Pending
        } else {
            InputObservation::Other
        }
    }
}

enum NativeDraft {
    Captured(String),
    ExistingStash,
    OccupiedCopy,
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
            aborted: None,
            discarded_aborted_payload: Arc::new(AtomicBool::new(false)),
            expected,
            directory,
            keys,
            steer: false,
            bracketed_paste: true,
            native_draft: Mutex::new(None),
        }
    }

    pub(super) fn with_aborted_payload(
        mut self,
        payload: Option<String>,
        discarded: Arc<AtomicBool>,
    ) -> Self {
        self.aborted = payload.map(|text| Mutex::new(AbortedMatcher::new(text)));
        self.discarded_aborted_payload = discarded;
        self
    }

    pub(super) fn with_steering(mut self, steer: bool) -> Self {
        self.steer = steer;
        self
    }

    pub(super) fn with_bracketed_paste(mut self, bracketed_paste: bool) -> Self {
        self.bracketed_paste = bracketed_paste;
        self
    }

    fn copy_lost_stash_marker(&self, screen: &str) -> bool {
        self.kind == AgentKind::Claude
            && self
                .native_draft
                .lock()
                .is_ok_and(|draft| matches!(draft.as_ref(), Some(NativeDraft::OccupiedCopy)))
            && !native_stash_present(screen)
    }

    fn copy_draft(&self, body: String) -> DraftPreservation {
        let restore_bytes = (!body.is_empty() && !opaque(&body)).then(|| {
            Bytes::from(crate::terminal::driver::input::paste(
                &body,
                self.bracketed_paste,
            ))
        });
        DraftPreservation::Copy {
            snapshot: body,
            restore_bytes,
        }
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
        if body.is_empty() && !allow_empty {
            return None;
        }
        if self.keys.len() == 3 {
            match self.kind {
                AgentKind::Claude if !native_stash_present(screen) => {
                    *self.native_draft.lock().ok()? = Some(NativeDraft::Captured(body.clone()));
                    return Some(DraftPreservation::Native {
                        key: self.keys[0].clone(),
                        directory: self.directory.clone(),
                        visible_draft: body,
                    });
                }
                AgentKind::Codex | AgentKind::Cursor | AgentKind::Kimi
                    if !body.is_empty() && !opaque(&body) =>
                {
                    return Some(DraftPreservation::Editor {
                        key: self.keys[1].clone(),
                        backspace: self.keys[2].clone(),
                        directory: self.directory.clone(),
                    });
                }
                _ => {}
            }
        }
        Some(self.copy_draft(body))
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
    fn waits_for_idle(&self) -> bool {
        self.owned.waits_for_idle()
    }
    fn provider_queues_input(&self, screen: &str, osc_title: &str, osc_progress: &str) -> bool {
        self.owned
            .provider_queues_input(screen, osc_title, osc_progress)
    }
    fn native_queue_visible(&self, screen: &str) -> bool {
        self.owned.native_queue_visible(screen)
    }
    fn record_submit_frame(&self, screen: &str) {
        self.owned.record_submit_frame(screen);
    }
    fn submitted_prompt_visible(&self, screen: &str) -> bool {
        self.owned.submitted_prompt_visible(screen)
    }
    fn retry_owned_enter(&self) -> bool {
        self.owned.retry_owned_enter()
    }
    fn native_draft_preserved(&self) -> bool {
        self.native_draft
            .lock()
            .map_or(true, |draft| draft.is_some())
    }
    fn clear_native_draft(&self) {
        if let Ok(mut draft) = self.native_draft.lock() {
            *draft = None;
        }
    }
    fn composer_copy_fallback(
        &self,
        screen: &str,
        cursor: Option<(u16, u16)>,
        pristine: bool,
    ) -> Option<(String, Option<Bytes>)> {
        let body = super::input_since_launch(self.kind, screen, cursor, pristine)?;
        let restore = (!body.is_empty() && !opaque(&body)).then(|| {
            Bytes::from(crate::terminal::driver::input::paste(
                &body,
                self.bracketed_paste,
            ))
        });
        Some((body, restore))
    }
    fn copy_recovery_directory(&self) -> Option<&std::path::Path> {
        Some(&self.directory)
    }
    fn confirm_consumption(&self) -> bool {
        self.owned.confirm_consumption()
    }
    fn recoverable_input(&self, screen: &str, cursor: Option<(u16, u16)>) -> bool {
        !self.copy_lost_stash_marker(screen) && self.owned.recoverable_input(screen, cursor)
    }
    fn owned_popup_escape(&self, screen: &str) -> Option<bytes::Bytes> {
        self.owned.owned_popup_escape(screen)
    }
    fn submission_available(&self, screen: &str) -> bool {
        super::submission_available(self.kind, screen)
    }
    fn paste_ready(&self, screen: &str) -> bool {
        if self.copy_lost_stash_marker(screen)
            || !super::paste_ready(self.kind, screen, &self.expected)
        {
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
            Some(NativeDraft::OccupiedCopy) | None => false,
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
        let observation = self.owned.observe_with_launch(screen, cursor, pristine);
        if observation == InputObservation::Pending && self.copy_lost_stash_marker(screen) {
            // The former stash may have promoted text equal to the prompt.
            // It cannot authorize a withdrawal or another Enter.
            InputObservation::Other
        } else {
            observation
        }
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
            .or_else(|| {
                invisible.then(|| DraftPreservation::Copy {
                    snapshot: String::new(),
                    restore_bytes: None,
                })
            })
    }
    fn native_stash_visible(&self, screen: &str) -> bool {
        native_stash_present(screen)
    }

    fn mark_occupied_stash_for_copy(&self, screen: &str) -> bool {
        if self.kind != AgentKind::Claude || !native_stash_present(screen) {
            return false;
        }
        self.native_draft.lock().is_ok_and(|mut draft| {
            *draft = Some(NativeDraft::OccupiedCopy);
            true
        })
    }

    fn copy_stash_key(&self) -> Option<Bytes> {
        (self.kind == AgentKind::Claude && self.keys.len() == 3).then(|| self.keys[0].clone())
    }

    fn copy_stash_payload_is_distinct(&self, screen: &str, cursor: Option<(u16, u16)>) -> bool {
        super::observe_with_cursor(self.kind, screen, &self.expected, cursor)
            == InputObservation::Other
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
            Some(NativeDraft::OccupiedCopy) => {
                self.owned.submitted_prompt_visible(screen)
                    || self.copy_stash_payload_is_distinct(screen, None)
            }
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

    fn observe_aborted_payload(
        &self,
        screen: &str,
        cursor: Option<(u16, u16)>,
        pristine: bool,
        unedited_stop: bool,
    ) -> InputObservation {
        self.aborted
            .as_ref()
            .and_then(|aborted| aborted.lock().ok())
            .map_or(InputObservation::Other, |mut aborted| {
                aborted.observe(self.kind, screen, cursor, pristine, unedited_stop)
            })
    }

    fn clearing_aborted(&self, _key_index: usize) {}

    fn aborted_cleanup_progress(&self) -> u64 {
        self.aborted
            .as_ref()
            .and_then(|aborted| aborted.lock().ok())
            .map_or(0, |aborted| aborted.progress)
    }

    fn mark_aborted_payload_discarded(&self) {
        self.discarded_aborted_payload
            .store(true, Ordering::Release);
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
