//! Durable request grouping, history caching, headers and age labels.
use super::super::render::{display, provider, wrap, wrap_ranges, Action};
use super::super::thumbnails::{Thumbnails, MAX_ROWS};
use super::{
    History, LayoutKey, Line, LinkScope, MarkdownSource, RowAnchor, RowKind, ThumbnailRow, Tone,
};
use crate::messaging::model::{
    AgentId, Author, BusState, Prompt, Request, RequestId, RequestPhase, Room, RoomId,
};
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

struct Exchange<'a> {
    prompt: &'a Prompt,
    requests: BTreeMap<AgentId, &'a Request>,
}

#[cfg(test)]
#[path = "../tests/history_group_test.rs"]
mod group_tests;

impl History {
    pub fn lines(
        &mut self,
        state: &BusState,
        room: &Room,
        width: u16,
        revision: u64,
        now: u64,
        thumbnails: &mut Thumbnails,
    ) -> &[Line] {
        static LINES: crate::utils::perf::Timer =
            crate::utils::perf::Timer::new("client.history.lines");
        static REBUILD: crate::utils::perf::Timer =
            crate::utils::perf::Timer::new("client.history.rebuild");
        let _lines_timer = LINES.start();
        if self.source != Some((revision, room.id)) {
            self.signature = signature(state, room);
            self.source = Some((revision, room.id));
        }
        let key = (
            self.signature,
            room.id,
            width,
            now / 60_000,
            thumbnails.layout_key(),
        );
        if self.key == Some(key) {
            return &self.lines;
        }
        let started = std::time::Instant::now();
        let reason = rebuild_reason(self.key, key);
        self.rendered = 0;
        let groups = state.groups();
        let exchanges = exchanges(state, room);
        let mut lines = Vec::new();
        let mut active_markdown = BTreeSet::new();
        for exchange in exchanges.into_values() {
            let prompt = exchange.prompt;
            self.push_prompt_lines(
                &mut lines,
                &mut active_markdown,
                state,
                room.id,
                prompt,
                width,
                now,
            );
            push_prompt_files(&mut lines, prompt, width, thumbnails);

            self.push_reply_lines(
                &mut lines,
                &mut active_markdown,
                state,
                room,
                &exchange,
                &groups,
                width,
            );
            lines.push(blank_row(RowAnchor::new(prompt.id, None, RowKind::Gap)));
        }
        self.room_markdown.insert(room.id, active_markdown);
        self.room_markdown.retain(|id, _| state.room(*id).is_some());
        let shown: BTreeSet<_> = self.room_markdown.values().flatten().copied().collect();
        self.markdown.retain(|source, _| shown.contains(source));
        self.key = Some(key);
        self.lines = lines;
        REBUILD.record_op(
            started.elapsed(),
            &format!(
                "room={} reason={reason} rows={} rendered={}",
                room.id.0,
                self.lines.len(),
                self.rendered
            ),
        );
        &self.lines
    }
    fn push_prompt_lines(
        &mut self,
        lines: &mut Vec<Line>,
        active_markdown: &mut BTreeSet<MarkdownSource>,
        state: &BusState,
        room: RoomId,
        prompt: &Prompt,
        width: u16,
        now: u64,
    ) {
        let mut header = vec![
            (
                participant_label(state, &prompt.author),
                participant_tone(&prompt.author),
            ),
            (" → ".into(), Tone::Muted),
        ];
        for (index, id) in prompt.recipient_ids.iter().enumerate() {
            if index > 0 {
                header.push((", ".into(), Tone::Muted));
            }
            header.push((state.agent_label(*id), Tone::Agent(*id)));
        }
        if prompt.recipient_ids.is_empty() {
            header.push(("You".into(), Tone::You));
        }
        let attention = state
            .is_developer_attention_author(room, &prompt.author)
            .then(|| crate::messaging::attention::text(&prompt.text))
            .flatten();
        if attention.is_some() {
            header.push(("  [Needs developer]".into(), Tone::Text));
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
        let prompt_source = MarkdownSource::Prompt(prompt.id);
        active_markdown.insert(prompt_source);
        let scope = link_scope(state, &prompt.author);
        let block = self.markdown_block(prompt_source, attention.unwrap_or(&prompt.text));
        let cold = !block.is_laid_out(width);
        lines.extend(super::wrapped_links::scoped_lines(
            block.lines(
                width,
                prompt_source,
                "",
                RowAnchor::new(prompt.id, None, RowKind::PromptBody),
            ),
            scope,
        ));
        self.rendered += usize::from(cold);
    }

    fn push_reply_lines(
        &mut self,
        lines: &mut Vec<Line>,
        active_markdown: &mut BTreeSet<MarkdownSource>,
        state: &BusState,
        room: &Room,
        exchange: &Exchange<'_>,
        groups: &BTreeMap<RequestId, Vec<RequestId>>,
        width: u16,
    ) {
        let prompt = exchange.prompt;
        let mut replied = false;
        for agent_id in &prompt.recipient_ids {
            let provider_kind = state
                .agent(*agent_id)
                .map(|a| a.provider)
                .or_else(|| state.retired_agent(*agent_id).and_then(|a| a.provider));
            let request = exchange.requests.get(agent_id).copied();
            // A group's messages share one reply per room, shown under that
            // room's newest visible message even when the group spans MASTER.
            if let Some(request) = request {
                let members = groups
                    .get(&request.group.unwrap_or(request.id))
                    .map_or(&[][..], Vec::as_slice);
                let newest = members.iter().rev().find(|id| {
                    state
                        .request(**id)
                        .is_some_and(|member| member.room_id == room.id)
                });
                if newest.is_some_and(|last| *last != request.id) {
                    continue;
                }
            }
            let final_reply = request
                .filter(|request| request.phase == RequestPhase::Completed)
                .and_then(|request| request.pending_final.as_ref());
            let legacy_reply = request
                .is_none()
                .then(|| room.latest_replies.get(agent_id))
                .flatten();
            let (text, quote) = if let Some(reply) = final_reply {
                (reply.text.as_str(), request.map(|request| request.id))
            } else if let Some(reply) = legacy_reply {
                (reply.text.as_str(), Some(reply.request_id))
            } else {
                ("…", None)
            };
            // One blank row between replies, as at every other block
            // boundary; the exchange's own Gap row follows the last one.
            if replied {
                lines.push(blank_row(RowAnchor::new(
                    prompt.id,
                    Some(*agent_id),
                    RowKind::Gap,
                )));
            }
            replied = true;
            let attention = state
                .is_developer_attention_author(room.id, &Author::Agent(*agent_id))
                .then(|| crate::messaging::attention::text(text))
                .flatten();
            let mut reply_header = vec![(state.agent_label(*agent_id), Tone::Agent(*agent_id))];
            if let Some(kind) = provider_kind {
                reply_header.push((format!("  {}", provider(kind)), Tone::Muted));
            }
            if attention.is_some() {
                reply_header.push(("  [Needs developer]".into(), Tone::Text));
            }
            let text = attention.unwrap_or(text);
            lines.extend(wrap_header(
                reply_header,
                width,
                "    ",
                RowAnchor::new(prompt.id, Some(*agent_id), RowKind::ReplyHeader),
            ));
            let reply_anchor = RowAnchor::new(prompt.id, Some(*agent_id), RowKind::ReplyBody);
            if let Some(request) = quote {
                let source = MarkdownSource::Reply(request);
                active_markdown.insert(source);
                let scope = link_scope(state, &Author::Agent(*agent_id));
                let block = self.markdown_block(source, text);
                let cold = !block.is_laid_out(width);
                lines.extend(super::wrapped_links::scoped_lines(
                    block.lines(width, source, "    ", reply_anchor),
                    scope,
                ));
                self.rendered += usize::from(cold);
                lines.push(Line {
                    text: "    Quote".into(),
                    action: Some(Action::Quote(request)),
                    tone: Tone::Muted,
                    spans: Vec::new(),
                    styles: Vec::new(),
                    links: None,
                    thumbnail: None,
                    raw_markdown: None,
                    continued: false,
                    rejoin_space: false,
                    copy_from: 0,
                    anchor: RowAnchor::new(prompt.id, Some(*agent_id), RowKind::Quote),
                });
            } else {
                push_body(lines, text, width, "    ", reply_anchor);
            }
        }
    }
}

fn push_body(lines: &mut Vec<Line>, text: &str, width: u16, indent: &str, anchor: RowAnchor) {
    let rows = wrap_ranges(text, width.saturating_sub(indent.len() as u16));
    lines.extend(rows.iter().enumerate().map(|(index, row)| Line {
        text: format!("{indent}{}", display(&text[row.clone()])),
        action: None,
        tone: Tone::Text,
        spans: Vec::new(),
        styles: Vec::new(),
        links: None,
        thumbnail: None,
        raw_markdown: None,
        continued: index > 0 && rows[index - 1].end == row.start,
        rejoin_space: false,
        copy_from: indent.len(),
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
                links: None,
                thumbnail: None,
                raw_markdown: None,
                continued: index > 0,
                rejoin_space: false,
                copy_from: indent.len(),
                anchor: RowAnchor {
                    position: index,
                    ..anchor
                },
            }
        })
        .collect()
}

/// The room's history entries in display order: requests grouped by prompt,
/// then notices and any saved latest-only prompt.
fn exchanges<'a>(state: &'a BusState, room: &'a Room) -> BTreeMap<(u64, u64), Exchange<'a>> {
    let mut exchanges = BTreeMap::new();
    // Room messages, including Bus notices, share prompt/reply history.
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
    // Bus notices for the developer read like messages addressed to nobody.
    for prompt in state.notices(room.id) {
        exchanges.insert(
            (prompt.submitted_at_ms, prompt.id.0),
            Exchange {
                prompt,
                requests: BTreeMap::new(),
            },
        );
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
    exchanges
}

fn blank_row(anchor: RowAnchor) -> Line {
    Line {
        text: String::new(),
        action: None,
        tone: Tone::Text,
        spans: Vec::new(),
        styles: Vec::new(),
        links: None,
        thumbnail: None,
        raw_markdown: None,
        continued: false,
        rejoin_space: false,
        copy_from: 0,
        anchor,
    }
}

/// Which layout input changed, for the rebuild's slow-operation detail.
fn rebuild_reason(previous: Option<LayoutKey>, next: LayoutKey) -> &'static str {
    let Some((signature, room, width, minute, thumbnails)) = previous else {
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
    } else if minute != next.3 {
        "minute"
    } else {
        "unchanged"
    }
}

fn participant_label(state: &BusState, participant: &Author) -> String {
    match participant {
        Author::Developer => "You".into(),
        Author::Bus => crate::messaging::model::BUS_SENDER.into(),
        Author::Agent(id) => state.agent_label(*id),
    }
}

fn link_scope(state: &BusState, author: &Author) -> LinkScope {
    let cwd = match author {
        Author::Agent(id) => state
            .agent(*id)
            .map(|agent| agent.cwd.as_path())
            .or_else(|| {
                state
                    .retired_agent(*id)
                    .and_then(|agent| agent.cwd.as_deref())
            })
            .map(Arc::from),
        Author::Developer | Author::Bus => None,
    };
    LinkScope {
        cwd,
        wrapped: Vec::new(),
    }
}

fn participant_tone(participant: &Author) -> Tone {
    match participant {
        Author::Agent(id) => Tone::Agent(*id),
        Author::Developer => Tone::You,
        Author::Bus => Tone::Bus,
    }
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
        request.group.map(|lead| lead.0).hash(&mut hash);
        if request.phase == RequestPhase::Completed {
            request
                .pending_final
                .as_ref()
                .map(|r| r.received_at_ms)
                .hash(&mut hash);
        }
    }
    room.latest_prompt.as_ref().map(|p| p.id.0).hash(&mut hash);
    state
        .notices(room.id)
        .last()
        .map(|p| p.id.0)
        .hash(&mut hash);
    for reply in room.latest_replies.values() {
        (reply.request_id.0, reply.received_at_ms).hash(&mut hash);
    }
    // Headers name prompt authors and recipients from any room (a work room's
    // MASTER orchestrator), so their names and colors key the layout too.
    let mut agents: BTreeSet<AgentId> = state
        .agents()
        .filter(|a| a.room_id == room.id)
        .map(|a| a.id)
        .collect();
    let prompts = state
        .requests()
        .filter(|r| r.room_id == room.id)
        .map(|r| &r.prompt)
        .chain(state.notices(room.id))
        .chain(room.latest_prompt.as_ref());
    for prompt in prompts {
        agents.extend(prompt.recipient_ids.iter().copied());
        if let Author::Agent(id) = prompt.author {
            agents.insert(id);
        }
    }
    for id in agents {
        id.0.hash(&mut hash);
        state
            .agent(id)
            .map(|agent| (&agent.name, agent.color))
            .hash(&mut hash);
    }
    hash.finish()
}

pub(in crate::client::rooms) fn reply(
    state: &BusState,
    room: RoomId,
    id: RequestId,
) -> Option<(AgentId, &str)> {
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

pub(in crate::client::rooms) fn timestamp(at: u64, now: u64) -> String {
    if now.saturating_sub(at) > 86_400_000 {
        return "> 1 day".into();
    }
    i64::try_from(at / 1000)
        .ok()
        .and_then(crate::platform::local_datetime_at)
        .map(|local| format!("{:02}:{:02}", local.hour(), local.minute()))
        .unwrap_or_else(|| "--:--".into())
}

fn push_prompt_files(
    lines: &mut Vec<Line>,
    prompt: &Prompt,
    width: u16,
    thumbnails: &mut Thumbnails,
) {
    if prompt.files.is_empty() {
        return;
    }
    // Attachments belong to the message above them: they go before its
    // trailing blank row, and a blank row separates them from the replies.
    let trailing = lines
        .iter()
        .rev()
        .take_while(|line| line.text.is_empty() && line.thumbnail.is_none())
        .count();
    let gap = lines.split_off(lines.len() - trailing);
    for (index, path) in prompt.files.iter().enumerate() {
        let file_anchor = RowAnchor {
            position: index,
            ..RowAnchor::new(prompt.id, None, RowKind::File)
        };
        // With an image protocol the picture stands in for the file:
        // its rows open the file, and no name row follows. The name
        // shows only where no picture can be drawn.
        if let Some((cols, rows)) = thumbnails.size(path, width) {
            let shared: Arc<std::path::Path> = Arc::from(path.as_path());
            lines.extend((0..rows).map(|row| Line {
                text: String::new(),
                action: Some(Action::OpenFile(path.clone())),
                tone: Tone::Muted,
                spans: Vec::new(),
                styles: Vec::new(),
                links: None,
                thumbnail: Some(ThumbnailRow {
                    path: Arc::clone(&shared),
                    cols,
                    rows,
                    row,
                }),
                raw_markdown: None,
                continued: false,
                rejoin_space: false,
                copy_from: 0,
                anchor: RowAnchor {
                    kind: RowKind::Thumbnail,
                    position: index * usize::from(MAX_ROWS) + usize::from(row),
                    ..file_anchor
                },
            }));
            continue;
        }
        let name = display(&path.file_name().unwrap_or_default().to_string_lossy());
        lines.push(Line {
            styles: vec![(name.clone(), super::super::links::LINK)],
            text: name,
            action: Some(Action::OpenFile(path.clone())),
            tone: Tone::Text,
            spans: Vec::new(),
            links: None,
            thumbnail: None,
            raw_markdown: None,
            continued: false,
            rejoin_space: false,
            copy_from: 0,
            anchor: file_anchor,
        });
    }
    if gap.is_empty() {
        lines.push(Line {
            text: String::new(),
            action: None,
            tone: Tone::Text,
            spans: Vec::new(),
            styles: Vec::new(),
            links: None,
            thumbnail: None,
            raw_markdown: None,
            continued: false,
            rejoin_space: false,
            copy_from: 0,
            anchor: RowAnchor {
                position: prompt.files.len(),
                ..RowAnchor::new(prompt.id, None, RowKind::File)
            },
        });
    }
    lines.extend(gap);
}
