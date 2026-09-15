//! Room history is a projection of durable requests, not the latest-reply cache.
use super::render::{display, provider, wrap, wrap_ranges, Action};
use crate::bus::model::*;
use markdown_ratatui::{DocumentRow, LayoutOptions, MarkdownView, Theme, ViewState};
use ratatui::buffer::{Buffer, CellWidth};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::StatefulWidget;
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
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
    /// Durable Markdown source for a rendered agent-reply row. Every row of
    /// one reply shares the same allocation so selection can copy the source
    /// once instead of reconstructing it from the display projection.
    pub raw_markdown: Option<(RequestId, Arc<str>)>,
    /// Soft-wrapped continuation of the previous row, rejoined when copied.
    pub continued: bool,
    anchor: RowAnchor,
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

#[derive(Default)]
pub(super) struct History {
    key: Option<(u64, RoomId, u16, u64)>,
    source: Option<(u64, RoomId)>,
    signature: u64,
    lines: Vec<Line>,
    markdown: BTreeMap<RequestId, MarkdownReply>,
}

struct MarkdownReply {
    source: Arc<str>,
    view: Option<MarkdownView>,
    width: Option<u16>,
    lines: Vec<Line>,
}

struct Exchange<'a> {
    prompt: &'a Prompt,
    requests: BTreeMap<AgentId, &'a Request>,
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

    pub fn lines(
        &mut self,
        state: &BusState,
        room: &Room,
        width: u16,
        revision: u64,
        now: u64,
    ) -> &[Line] {
        if self.source != Some((revision, room.id)) {
            self.signature = signature(state, room);
            self.source = Some((revision, room.id));
        }
        let key = (self.signature, room.id, width, now / 60_000);
        if self.key == Some(key) {
            return &self.lines;
        }
        let mut exchanges = BTreeMap::new();
        for request in state.requests().filter(|r| r.room_id == room.id) {
            let prompt = &request.prompt;
            exchanges
                .entry((prompt.submitted_at_ms, prompt.id.0))
                .or_insert_with(|| Exchange {
                    prompt,
                    requests: BTreeMap::new(),
                })
                .requests
                .insert(request.agent_id, request);
        }
        // Retain compatibility with saved latest-only records.
        if let Some(prompt) = room.latest_prompt.as_ref().filter(|prompt| {
            !exchanges
                .values()
                .any(|exchange| exchange.prompt.id == prompt.id)
        }) {
            exchanges.insert(
                (prompt.submitted_at_ms, prompt.id.0),
                Exchange {
                    prompt,
                    requests: BTreeMap::new(),
                },
            );
        }
        let mut lines = Vec::new();
        let mut active_markdown = BTreeSet::new();
        for exchange in exchanges.into_values() {
            let prompt = exchange.prompt;
            let mut header = vec![("You".into(), Tone::You), (" → ".into(), Tone::Muted)];
            for (index, agent) in prompt
                .recipient_ids
                .iter()
                .filter_map(|id| state.agent(*id))
                .enumerate()
            {
                if index > 0 {
                    header.push((", ".into(), Tone::Muted));
                }
                header.push((agent.name.clone(), Tone::Agent(agent.id)));
            }
            header.push((
                format!("  {}", timestamp(prompt.submitted_at_ms, now)),
                Tone::Muted,
            ));
            lines.extend(wrap_header(
                header,
                width,
                "",
                RowAnchor::new(prompt.id, None, RowKind::PromptHeader),
            ));
            push_body(
                &mut lines,
                &prompt.text,
                width,
                "",
                RowAnchor::new(prompt.id, None, RowKind::PromptBody),
            );
            for (index, path) in prompt.files.iter().enumerate() {
                lines.push(Line {
                    text: format!(
                        "[{}]",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ),
                    action: Some(Action::FileDetail(path.clone())),
                    tone: Tone::Muted,
                    spans: Vec::new(),
                    styles: Vec::new(),
                    raw_markdown: None,
                    continued: false,
                    anchor: RowAnchor {
                        position: index,
                        ..RowAnchor::new(prompt.id, None, RowKind::File)
                    },
                });
            }

            for agent_id in &prompt.recipient_ids {
                let Some(agent) = state.agent(*agent_id) else {
                    continue;
                };
                let request = exchange.requests.get(agent_id).copied();
                let final_reply = request
                    .filter(|request| request.phase == RequestPhase::Completed)
                    .and_then(|request| request.pending_final.as_ref());
                let legacy_reply = request
                    .is_none()
                    .then(|| room.latest_replies.get(agent_id))
                    .flatten();
                let (text, at, quote) = if let Some(reply) = final_reply {
                    (
                        reply.text.as_str(),
                        Some(reply.received_at_ms),
                        request.map(|request| request.id),
                    )
                } else if let Some(reply) = legacy_reply {
                    (
                        reply.text.as_str(),
                        Some(reply.received_at_ms),
                        Some(reply.request_id),
                    )
                } else {
                    ("…", None, None)
                };
                let mut reply_header = vec![
                    (agent.name.clone(), Tone::Agent(agent.id)),
                    (format!("  {}", provider(agent.provider)), Tone::Muted),
                ];
                if let Some(at) = at {
                    reply_header.push((format!("  {}", timestamp(at, now)), Tone::Muted));
                }
                lines.extend(wrap_header(
                    reply_header,
                    width,
                    "    ",
                    RowAnchor::new(prompt.id, Some(*agent_id), RowKind::ReplyHeader),
                ));
                let reply_anchor = RowAnchor::new(prompt.id, Some(*agent_id), RowKind::ReplyBody);
                if let Some(request) = quote {
                    active_markdown.insert(request);
                    let markdown = self
                        .markdown
                        .entry(request)
                        .or_insert_with(|| MarkdownReply::new(text));
                    if markdown.source.as_ref() != text {
                        *markdown = MarkdownReply::new(text);
                    }
                    lines.extend(
                        markdown
                            .lines(width, request, "    ", reply_anchor)
                            .iter()
                            .cloned(),
                    );
                    lines.push(Line {
                        text: "    Quote".into(),
                        action: Some(Action::Quote(request)),
                        tone: Tone::Muted,
                        spans: Vec::new(),
                        styles: Vec::new(),
                        raw_markdown: None,
                        continued: false,
                        anchor: RowAnchor::new(prompt.id, Some(*agent_id), RowKind::Quote),
                    });
                } else {
                    push_body(&mut lines, text, width, "    ", reply_anchor);
                }
            }
            lines.push(Line {
                text: String::new(),
                action: None,
                tone: Tone::Text,
                spans: Vec::new(),
                styles: Vec::new(),
                raw_markdown: None,
                continued: false,
                anchor: RowAnchor::new(prompt.id, None, RowKind::Gap),
            });
        }
        self.markdown
            .retain(|request, _| active_markdown.contains(request));
        self.key = Some(key);
        self.lines = lines;
        &self.lines
    }
}

impl MarkdownReply {
    fn new(source: &str) -> Self {
        let raw: Arc<str> = Arc::from(source);
        let view = match MarkdownView::new(source) {
            Ok(mut view) => {
                view.set_options(LayoutOptions {
                    theme: Theme {
                        text: Style::default(),
                        heading: Style::new().bold(),
                        link: Style::new().fg(Color::Cyan).underlined(),
                        code: Style::new().fg(Color::Cyan),
                        muted: Style::new().add_modifier(Modifier::DIM),
                        selected: Style::default(),
                    },
                    ..LayoutOptions::default()
                });
                Some(view)
            }
            Err(error) => {
                tracing::warn!(event = "bus.markdown.parse_failed", %error);
                None
            }
        };
        Self {
            source: raw,
            view,
            width: None,
            lines: Vec::new(),
        }
    }

    fn lines(
        &mut self,
        width: u16,
        request: RequestId,
        indent: &str,
        anchor: RowAnchor,
    ) -> &[Line] {
        if self.width == Some(width) {
            return &self.lines;
        }
        let content_width = width.saturating_sub(indent.len() as u16).max(1);
        self.lines = self
            .view
            .as_mut()
            .and_then(|view| {
                prepared_markdown_lines(view, content_width, request, &self.source, indent, anchor)
            })
            .unwrap_or_else(|| {
                literal_reply_lines(&self.source, content_width, request, indent, anchor)
            });
        self.width = Some(width);
        &self.lines
    }
}

fn prepared_markdown_lines(
    view: &mut MarkdownView,
    width: u16,
    request: RequestId,
    source: &Arc<str>,
    indent: &str,
    anchor: RowAnchor,
) -> Option<Vec<Line>> {
    let layout = match view.prepare(width) {
        Ok(layout) => layout,
        Err(error) => {
            tracing::warn!(event = "bus.markdown.layout_failed", %error);
            return None;
        }
    };
    let line_count = layout.line_count();
    let mut lines = Vec::with_capacity(line_count);
    let mut first = 0;
    while first < line_count {
        // Bound the temporary cell buffer independently of reply length. The
        // prepared layout remains cached; chunks only adapt its styled runs to
        // the room history's existing row representation.
        const MAX_BUFFER_CELLS: usize = 256 * 1024;
        let rows_per_chunk = (MAX_BUFFER_CELLS / usize::from(width.max(1))).max(1);
        let height = (line_count - first)
            .min(rows_per_chunk)
            .min(usize::from(u16::MAX)) as u16;
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        let mut state = ViewState::default();
        state.scroll_to(DocumentRow::new(first));
        layout.widget().render(area, &mut buffer, &mut state);
        for offset in 0..height {
            let position = first + usize::from(offset);
            lines.push(line_from_buffer(
                &buffer,
                offset,
                width,
                request,
                source,
                indent,
                position > 0,
                RowAnchor { position, ..anchor },
            ));
        }
        first += usize::from(height);
    }
    Some(lines)
}

#[allow(clippy::too_many_arguments)]
fn line_from_buffer(
    buffer: &Buffer,
    row: u16,
    width: u16,
    request: RequestId,
    source: &Arc<str>,
    indent: &str,
    continued: bool,
    anchor: RowAnchor,
) -> Line {
    let mut text = indent.to_owned();
    let mut styles: Vec<(String, Style)> = if indent.is_empty() {
        Vec::new()
    } else {
        vec![(indent.to_owned(), Style::default())]
    };
    let mut column = 0u16;
    while column < width {
        let cell = &buffer[(column, row)];
        let symbol = cell.symbol();
        let mut style = cell.style();
        // A scratch buffer represents unspecified colors as Reset. Treat those
        // as transparent so Markdown modifiers patch the Bus room palette
        // instead of replacing it with the user's terminal defaults.
        if style.fg == Some(Color::Reset) {
            style.fg = None;
        }
        if style.bg == Some(Color::Reset) {
            style.bg = None;
        }
        if style.underline_color == Some(Color::Reset) {
            style.underline_color = None;
        }
        if !symbol.is_empty() {
            text.push_str(symbol);
            if let Some((run, _)) = styles.last_mut().filter(|(_, current)| *current == style) {
                run.push_str(symbol);
            } else {
                styles.push((symbol.to_owned(), style));
            }
        }
        column = column.saturating_add(cell.cell_width().max(1));
    }
    while text.len() > indent.len() && text.ends_with(' ') {
        text.pop();
        if let Some((run, _)) = styles.last_mut() {
            debug_assert!(run.ends_with(' '));
            run.pop();
            if run.is_empty() {
                styles.pop();
            }
        }
    }
    Line {
        text,
        action: None,
        tone: Tone::Text,
        spans: Vec::new(),
        styles,
        raw_markdown: Some((request, Arc::clone(source))),
        continued,
        anchor,
    }
}

fn literal_reply_lines(
    source: &Arc<str>,
    width: u16,
    request: RequestId,
    indent: &str,
    anchor: RowAnchor,
) -> Vec<Line> {
    let rows = wrap_ranges(source, width);
    rows.iter()
        .enumerate()
        .map(|(index, row)| Line {
            text: format!("{indent}{}", display(&source[row.clone()])),
            action: None,
            tone: Tone::Text,
            spans: Vec::new(),
            styles: Vec::new(),
            raw_markdown: Some((request, Arc::clone(source))),
            continued: index > 0 && rows[index - 1].end == row.start,
            anchor: RowAnchor {
                position: row.start,
                ..anchor
            },
        })
        .collect()
}

// Keep styling attached to header fields, not matches against arbitrary message
// text. Wrapping preserves every byte of the sanitized source, including UTF-8.
fn push_body(lines: &mut Vec<Line>, text: &str, width: u16, indent: &str, anchor: RowAnchor) {
    let rows = wrap_ranges(text, width.saturating_sub(indent.len() as u16));
    lines.extend(rows.iter().enumerate().map(|(index, row)| Line {
        text: format!("{indent}{}", display(&text[row.clone()])),
        action: None,
        tone: Tone::Text,
        spans: Vec::new(),
        styles: Vec::new(),
        raw_markdown: None,
        continued: index > 0 && rows[index - 1].end == row.start,
        anchor: RowAnchor {
            position: row.start,
            ..anchor
        },
    }));
}

fn wrap_header(
    spans: Vec<(String, Tone)>,
    width: u16,
    indent: &str,
    anchor: RowAnchor,
) -> Vec<Line> {
    let mut source = String::new();
    let mut ranges = Vec::new();
    for (text, tone) in spans {
        let start = source.len();
        source.push_str(&display(&text));
        ranges.push((start..source.len(), tone));
    }
    let mut offset = 0;
    wrap(&source, width.saturating_sub(indent.len() as u16))
        .into_iter()
        .enumerate()
        .map(|(index, text)| {
            let end = offset + text.len();
            let spans: Vec<_> = ranges
                .iter()
                .filter_map(|(range, tone)| {
                    let start = range.start.max(offset);
                    let stop = range.end.min(end);
                    (start < stop).then(|| (source[start..stop].to_owned(), *tone))
                })
                .collect();
            offset = end;
            let mut line_spans = vec![(indent.to_owned(), Tone::Muted)];
            line_spans.extend(spans);
            Line {
                text: format!("{indent}{text}"),
                action: None,
                tone: Tone::Muted,
                spans: line_spans,
                styles: Vec::new(),
                raw_markdown: None,
                continued: index > 0,
                anchor: RowAnchor {
                    position: index,
                    ..anchor
                },
            }
        })
        .collect()
}

// Prompts and completed finals are immutable after creation. Check their
// identity/settlement metadata, not their potentially large text, when a
// global poll/draft revision arrives. Only a changed room history, names,
// width, or age label requires sorting and wrapping those immutable bodies.
fn signature(state: &BusState, room: &Room) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    for request in state.requests().filter(|r| r.room_id == room.id) {
        request.id.0.hash(&mut hash);
        request.prompt.id.0.hash(&mut hash);
        request.prompt.submitted_at_ms.hash(&mut hash);
        if request.phase == RequestPhase::Completed {
            request
                .pending_final
                .as_ref()
                .map(|r| r.received_at_ms)
                .hash(&mut hash);
        }
    }
    room.latest_prompt.as_ref().map(|p| p.id.0).hash(&mut hash);
    for reply in room.latest_replies.values() {
        (reply.request_id.0, reply.received_at_ms).hash(&mut hash);
    }
    for agent in state.agents().filter(|a| a.room_id == room.id) {
        (agent.id.0, &agent.name, agent.color).hash(&mut hash);
    }
    hash.finish()
}

pub(super) fn reply(state: &BusState, room: RoomId, id: RequestId) -> Option<(AgentId, &str)> {
    if let Some(request) = state
        .request(id)
        .filter(|r| r.room_id == room && r.phase == RequestPhase::Completed)
    {
        if let Some(reply) = &request.pending_final {
            return Some((request.agent_id, &reply.text));
        }
    }
    state
        .room(room)?
        .latest_replies
        .values()
        .find(|r| r.request_id == id)
        .map(|r| (r.agent_id, r.text.as_str()))
}

pub(super) fn timestamp(at: u64, now: u64) -> String {
    if now.saturating_sub(at) > 86_400_000 {
        return "> 1 day".into();
    }
    i64::try_from(at / 1000)
        .ok()
        .and_then(crate::platform::local_datetime_at)
        .map(|local| format!("{:02}:{:02}", local.hour(), local.minute()))
        .unwrap_or_else(|| "--:--".into())
}

#[cfg(test)]
mod tests {
    use super::*;
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
            .lines(&state, state.room(room).unwrap(), 80, 1, 1000)
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
                    .lines(&state, state.room(room).unwrap(), 80, revision, 1000)
                    .as_ptr(),
                original
            );
        }
        state.rename_agent(agent, "renamed").unwrap();
        assert!(history
            .lines(&state, state.room(room).unwrap(), 80, 31, 1000)
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
