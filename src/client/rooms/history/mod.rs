//! Room history is a projection of durable requests, not the latest-reply cache.
mod exchange;
mod markdown;

use super::render::Action;
use crate::messaging::model::{AgentId, PromptId, RequestId, RoomId};
pub(super) use exchange::reply;
#[cfg(test)]
use exchange::timestamp;
use markdown::MarkdownBlock;
use ratatui::style::Style;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Copy)]
pub(super) enum Tone {
    Text,
    Muted,
    You,
    Agent(AgentId),
}

#[derive(Clone)]
pub(super) struct Line {
    pub text: String,
    pub action: Option<Action>,
    pub tone: Tone,
    pub spans: Vec<(String, Tone)>,
    pub styles: Vec<(String, Style)>,
    /// Durable Markdown source for a rendered prompt or agent-reply row. Every
    /// row of one message shares the same allocation. Selection no longer
    /// copies it (a drag copies exactly what it covers); it identifies a row's
    /// message and lets tests prove a rendering is reused.
    #[cfg_attr(not(test), allow(dead_code))]
    pub raw_markdown: Option<(MarkdownSource, Arc<str>)>,
    /// Soft-wrapped continuation of the previous row, rejoined when copied.
    pub continued: bool,
    /// The wrap that `continued` rejoins dropped a space (rendered Markdown
    /// trims it), so copying puts one back.
    pub rejoin_space: bool,
    /// Bytes of layout indent at the start of `text`. They are not message
    /// text, so copying skips them.
    pub copy_from: usize,
    /// Row `row` of an image thumbnail drawn over these blank cells.
    pub thumbnail: Option<ThumbnailRow>,
    anchor: RowAnchor,
}

#[derive(Clone)]
pub(super) struct ThumbnailRow {
    pub path: Arc<std::path::Path>,
    pub cols: u16,
    pub rows: u16,
    pub row: u16,
}

/// Identifies one rendered Markdown message in the room history.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum MarkdownSource {
    Prompt(PromptId),
    Reply(RequestId),
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) struct RowAnchor {
    prompt: PromptId,
    agent: Option<AgentId>,
    kind: RowKind,
    position: usize,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum RowKind {
    PromptHeader,
    PromptBody,
    File,
    Thumbnail,
    ReplyHeader,
    ReplyBody,
    Quote,
    Gap,
}

impl RowAnchor {
    fn new(prompt: PromptId, agent: Option<AgentId>, kind: RowKind) -> Self {
        Self {
            prompt,
            agent,
            kind,
            position: 0,
        }
    }
}

/// Inputs to a history layout: content signature, room, width, age-label
/// minute, and thumbnail cell size.
type LayoutKey = (u64, RoomId, u16, u64, Option<(u32, u32)>);

#[derive(Default)]
pub(super) struct History {
    key: Option<LayoutKey>,
    source: Option<(u64, RoomId)>,
    signature: u64,
    lines: Vec<Line>,
    markdown: BTreeMap<MarkdownSource, MarkdownBlock>,
}

impl History {
    /// Rows from the latest `lines` call, as currently displayed.
    pub fn cached(&self) -> &[Line] {
        &self.lines
    }

    pub fn anchor_at(&self, index: usize) -> Option<RowAnchor> {
        self.lines.get(index).map(|line| line.anchor)
    }

    pub fn index_of(&self, anchor: RowAnchor) -> Option<usize> {
        self.lines.iter().position(|line| line.anchor == anchor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::model::{BusState, Provider};
    #[test]
    fn routine_poll_revisions_and_draft_edits_reuse_history_lines() {
        let mut state = BusState::default();
        let room = state.create_room("test").unwrap();
        let agent = state
            .create_agent(room, "agent", Provider::Codex, "/project".into(), None)
            .unwrap();
        state.set_draft_recipients(room, [agent]).unwrap();
        for i in 0..100 {
            state
                .set_draft_text(
                    room,
                    &format!("request {i}\n{}", "saved history ".repeat(100)),
                )
                .unwrap();
            state.submit_draft(room, i + 1).unwrap();
        }
        let mut history = History::default();
        let original = history
            .lines(
                &state,
                state.room(room).unwrap(),
                80,
                1,
                1000,
                &mut Default::default(),
            )
            .as_ptr();
        for revision in 2..=30 {
            state
                .set_draft_text(room, "draft edits do not change history")
                .unwrap();
            state
                .set_room_notes(room, "notes do not change history")
                .unwrap();
            assert_eq!(
                history
                    .lines(
                        &state,
                        state.room(room).unwrap(),
                        80,
                        revision,
                        1000,
                        &mut Default::default()
                    )
                    .as_ptr(),
                original
            );
        }
        state.rename_agent(agent, "renamed").unwrap();
        assert!(history
            .lines(
                &state,
                state.room(room).unwrap(),
                80,
                31,
                1000,
                &mut Default::default()
            )
            .iter()
            .any(|line| line.text.contains("renamed")));
    }
    #[test]
    fn day_cutoff_is_elapsed_time_not_midnight() {
        let at = 1_800_000_000_000;
        assert_ne!(timestamp(at, at + 86_400_000), "> 1 day");
        assert_eq!(timestamp(at, at + 86_400_001), "> 1 day");
        assert_eq!(timestamp(at, at + 60_000), timestamp(at, at));
        assert_eq!(timestamp(at, at - 1), timestamp(at, at));
    }
}
