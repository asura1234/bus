//! Room history is a projection of durable requests, not the latest-reply cache.
mod background;
mod exchange;
mod groups;
mod markdown;
mod window;
mod wrapped_links;

// Separate the costs of a rebuild so a slow operation identifies its phase.
mod phases {
    use crate::utils::perf::Timer;
    pub(super) static SIGNATURE: Timer = Timer::new("client.history.signature");
    pub(super) static GROUPING: Timer = Timer::new("client.history.grouping");
    pub(super) static PARSE: Timer = Timer::new("client.history.markdown.parse");
    pub(super) static LAYOUT: Timer = Timer::new("client.history.markdown.layout");
    pub(super) static PREPARE: Timer = Timer::new("client.history.markdown.prepare");
    pub(super) static PAINT: Timer = Timer::new("client.history.markdown.paint");
    pub(super) static ROWS: Timer = Timer::new("client.history.markdown.rows");
    pub(super) static LINKS: Timer = Timer::new("client.history.links");
    pub(super) static HEADER: Timer = Timer::new("client.history.header");
    pub(super) static WRAP: Timer = Timer::new("client.history.wrap");
    pub(super) static THUMBNAILS: Timer = Timer::new("client.history.thumbnails");
}

use super::{render::Action, LocalSendStatus};
use crate::messaging::model::{AgentId, PromptId, RequestId, RoomId};
#[cfg(test)]
use exchange::timestamp;
pub(super) use exchange::{reply, visible_request};
use markdown::MarkdownBlock;
use ratatui::style::Style;
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Debug, Eq, PartialEq))]
pub(super) enum Tone {
    Text,
    Muted,
    You,
    /// The grey "bus" sender of Bus-generated notices.
    Bus,
    Agent(AgentId),
}

#[derive(Clone)]
#[cfg_attr(test, derive(Debug, Eq, PartialEq))]
pub(super) struct Line {
    pub text: String,
    /// Addressing derived from this message's author, recipients and explicit
    /// developer mentions before rendering. It never depends on header text.
    pub to_developer: bool,
    /// True for rows of developer-authored messages, including attachments.
    pub from_developer: bool,
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
    /// Message text rows, whose URLs and file paths are clickable.
    pub links: Option<LinkScope>,
    anchor: RowAnchor,
}

/// Relative paths in a message resolve against `cwd`, its authoring agent's
/// working directory; the developer and Bus have none.
#[derive(Clone)]
#[cfg_attr(test, derive(Debug, Eq, PartialEq))]
pub(super) struct LinkScope {
    pub cwd: Option<Arc<std::path::Path>>,
    /// Complete source tokens projected onto this row after wrapping.
    pub wrapped: Vec<super::links::Candidate>,
}

#[derive(Clone)]
#[cfg_attr(test, derive(Debug, Eq, PartialEq))]
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
#[cfg_attr(test, derive(Debug))]
pub(super) struct RowAnchor {
    prompt: PromptId,
    agent: Option<AgentId>,
    kind: RowKind,
    position: usize,
}

#[derive(Clone, Copy, Eq, PartialEq)]
#[cfg_attr(test, derive(Debug))]
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
/// minute, thumbnail cell size and developer-direction filters.
type LayoutKey = (
    u64,
    RoomId,
    u16,
    u64,
    Option<(u32, u32)>,
    crate::messaging::model::RoomHistoryFilters,
    u64,
);

pub(super) fn filter_lines(
    lines: Vec<Line>,
    filters: crate::messaging::model::RoomHistoryFilters,
) -> Vec<Line> {
    if filters == Default::default() {
        return lines;
    }
    let accepts = |line: &Line| {
        (filters.to_you && line.to_developer) || (filters.from_you && line.from_developer)
    };
    let accepted: Vec<bool> = lines.iter().map(accepts).collect();
    lines
        .iter()
        .enumerate()
        .filter(|(index, line)| {
            accepted[*index]
                || line.anchor.kind == RowKind::Gap
                    && (*index > 0 && accepted[index - 1]
                        || *index + 1 < accepted.len() && accepted[index + 1])
        })
        .fold(Vec::new(), |mut filtered, (_, line)| {
            if line.anchor.kind != RowKind::Gap
                || filtered
                    .last()
                    .is_some_and(|previous: &Line| previous.anchor.kind != RowKind::Gap)
            {
                filtered.push(line.clone());
            }
            filtered
        })
}

fn submission_signature(submissions: &BTreeMap<PromptId, LocalSendStatus>) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    for (prompt, status) in submissions {
        prompt.0.hash(&mut hash);
        match status {
            LocalSendStatus::Pending => 0_u8,
            LocalSendStatus::Failed => 1_u8,
        }
        .hash(&mut hash);
    }
    hash.finish()
}

/// Which layout input changed, for the rebuild's slow-operation detail.
fn rebuild_reason(previous: Option<LayoutKey>, next: LayoutKey) -> &'static str {
    let Some((signature, room, width, minute, thumbnails, filters, submissions)) = previous else {
        return "first";
    };
    if room != next.1 {
        "room"
    } else if width != next.2 {
        "width"
    } else if signature != next.0 {
        "content"
    } else if thumbnails != next.4 {
        "thumbnails"
    } else if filters != next.5 {
        "filters"
    } else if submissions != next.6 {
        "submissions"
    } else if minute != next.3 {
        "minute"
    } else {
        "unchanged"
    }
}

#[derive(Default)]
pub(super) struct History {
    key: Option<LayoutKey>,
    source: Option<(u64, RoomId)>,
    signature: u64,
    lines: Arc<[Line]>,
    displayed_filters: crate::messaging::model::RoomHistoryFilters,
    background: Option<background::Background>,
    cancellation: Option<background::Cancellation>,
    age_valid: std::ops::Range<u64>,
    markdown: BTreeMap<MarkdownSource, MarkdownBlock>,
    /// The Markdown each visited room showed at its last rebuild. Rendering
    /// a long room is the costly part, so a block stays cached while any
    /// visited room still shows it, not only the room on screen.
    room_markdown: BTreeMap<RoomId, BTreeSet<MarkdownSource>>,
    /// Markdown blocks laid out afresh during the current rebuild.
    rendered: usize,
    rebuilt: usize,
    projected: usize,
    window_room: Option<(RoomId, crate::messaging::model::RoomHistoryFilters)>,
    record_limit: usize,
    loaded_limit: usize,
    has_older: bool,
}

impl History {
    /// Rows from the latest `lines` call, as currently displayed.
    pub fn cached(&self) -> &[Line] {
        &self.lines
    }

    pub fn current_filters(&self) -> crate::messaging::model::RoomHistoryFilters {
        self.displayed_filters
    }

    fn reusable_layout(&self, key: LayoutKey, now: u64) -> bool {
        self.key.is_some_and(|previous| {
            previous.0 == key.0
                && previous.1 == key.1
                && previous.2 == key.2
                && previous.4 == key.4
                && previous.5 == key.5
                && previous.6 == key.6
                && (previous.3 == key.3 || self.age_valid.contains(&now))
        })
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
