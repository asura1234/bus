use crate::error::{invalid, not_found, transition_denied, Result};
use clap::ValueEnum;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use serde::{Deserialize, Serialize};
use std::fmt;

pub const ALL_COLUMNS: [Column; 7] = [
    Column::Todo,
    Column::Blocked,
    Column::Investigation,
    Column::Implementation,
    Column::Testing,
    Column::Review,
    Column::Done,
];

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ValueEnum,
)]
#[serde(rename_all = "lowercase")]
pub enum Column {
    Todo,
    Blocked,
    Investigation,
    Implementation,
    Testing,
    Review,
    Done,
}

impl Column {
    pub fn as_str(&self) -> &'static str {
        match self {
            Column::Todo => "todo",
            Column::Blocked => "blocked",
            Column::Investigation => "investigation",
            Column::Implementation => "implementation",
            Column::Testing => "testing",
            Column::Review => "review",
            Column::Done => "done",
        }
    }

    pub fn allowed_targets(self, investigation_only: bool) -> Vec<Column> {
        match self {
            Column::Blocked => ALL_COLUMNS
                .into_iter()
                .filter(|c| *c != Column::Blocked)
                .collect(),
            Column::Todo => vec![Column::Investigation, Column::Blocked],
            Column::Investigation => {
                if investigation_only {
                    vec![Column::Implementation, Column::Done, Column::Blocked]
                } else {
                    vec![Column::Implementation, Column::Blocked]
                }
            }
            Column::Implementation => vec![Column::Testing, Column::Blocked],
            Column::Testing => vec![Column::Review, Column::Implementation, Column::Blocked],
            Column::Review => vec![Column::Done, Column::Implementation, Column::Blocked],
            Column::Done => vec![Column::Blocked],
        }
    }

    pub fn validate_transition(
        self,
        to: Column,
        investigation_only: bool,
        ticket_id: &str,
    ) -> Result<()> {
        if self == to {
            let allowed = self
                .allowed_targets(investigation_only)
                .into_iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(transition_denied(format!(
                "ticket {ticket_id} is already in '{}'; allowed targets from '{}': [{allowed}]",
                self.as_str(),
                self.as_str()
            )));
        }
        let targets = self.allowed_targets(investigation_only);
        if targets.contains(&to) {
            Ok(())
        } else {
            let allowed = targets
                .into_iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let hint = if self == Column::Investigation && to == Column::Done && !investigation_only
            {
                " (ticket is not marked --investigation-only; normal flow requires investigation -> implementation -> testing -> review -> done)"
            } else {
                ""
            };
            Err(transition_denied(format!(
                "invalid transition for ticket {ticket_id}: '{}' -> '{}' is not allowed{hint}; allowed targets from '{}': [{allowed}]",
                self.as_str(),
                to.as_str(),
                self.as_str()
            )))
        }
    }
}

impl fmt::Display for Column {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ViewFilter {
    Day,
    Week,
    Session,
}

impl ViewFilter {
    pub fn as_str(&self) -> &'static str {
        match self {
            ViewFilter::Day => "day",
            ViewFilter::Week => "week",
            ViewFilter::Session => "session",
        }
    }

    pub fn matches(self, ticket: &Ticket, now_ms: i64) -> bool {
        let touched = ticket.updated_at_ms.max(ticket.created_at_ms);
        match self {
            ViewFilter::Session => true,
            ViewFilter::Day => {
                let start = local_midnight_ms(now_ms);
                touched >= start
            }
            ViewFilter::Week => {
                let start = local_midnight_ms(now_ms) - 6 * 86_400_000;
                touched >= start
            }
        }
    }
}

pub fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn local_midnight_ms(now_ms: i64) -> i64 {
    let secs = (now_ms / 1000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::localtime_r(&secs, &mut tm);
    }
    let elapsed_today_secs =
        (tm.tm_hour as i64) * 3600 + (tm.tm_min as i64) * 60 + (tm.tm_sec as i64);
    let secs_ms = (secs as i64) * 1000;
    secs_ms - elapsed_today_secs * 1000
}

fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn is_safe_url(url: &str) -> bool {
    let trimmed = url.trim();
    trimmed.starts_with("https://") || trimmed.starts_with("http://")
}

/// Pre-process bare http(s):// URLs outside Markdown `[text](url)` or `<url>` so
/// plain URLs in a description also render as clickable links.
fn autolink_bare_urls(src: &str) -> String {
    let mut out = String::with_capacity(src.len() + 16);
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if (src[i..].starts_with("https://") || src[i..].starts_with("http://"))
            && (i == 0 || !matches!(bytes[i - 1], b'(' | b'<' | b'"' | b'\''))
        {
            let mut end = i;
            while end < bytes.len()
                && !bytes[end].is_ascii_whitespace()
                && !matches!(bytes[end], b')' | b'>' | b']' | b'"' | b'\'')
            {
                end += 1;
            }
            while end > i && matches!(bytes[end - 1], b'.' | b',' | b';' | b':') {
                end -= 1;
            }
            let url = &src[i..end];
            out.push('<');
            out.push_str(url);
            out.push('>');
            i = end;
        } else {
            let ch = src[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// Render a one-sentence ticket description from Markdown into sanitized HTML:
/// - Raw HTML tags (`Event::Html`, `Event::InlineHtml`) are escaped as plain text, never injected.
/// - Safe `http://` and `https://` links render as `<a href="..." target="_blank" rel="noopener noreferrer">...</a>`.
/// - Unsafe link schemes (`javascript:`, `data:`, etc.) are stripped to plain text.
pub fn render_sanitized_markdown(markdown: &str) -> String {
    let prepared = autolink_bare_urls(markdown.trim());
    if prepared.is_empty() {
        return String::new();
    }
    let parser = Parser::new_ext(&prepared, Options::empty());
    let mut html = String::new();
    let mut link_stack: Vec<bool> = Vec::new();

    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {}
                Tag::Strong => html.push_str("<strong>"),
                Tag::Emphasis => html.push_str("<em>"),
                Tag::Link { dest_url, .. } => {
                    if is_safe_url(&dest_url) {
                        html.push_str("<a href=\"");
                        html.push_str(&escape_html(dest_url.trim()));
                        html.push_str("\" target=\"_blank\" rel=\"noopener noreferrer\">");
                        link_stack.push(true);
                    } else {
                        link_stack.push(false);
                    }
                }
                _ => {}
            },
            Event::End(tag_end) => match tag_end {
                TagEnd::Paragraph => {}
                TagEnd::Strong => html.push_str("</strong>"),
                TagEnd::Emphasis => html.push_str("</em>"),
                TagEnd::Link if link_stack.pop().unwrap_or(false) => {
                    html.push_str("</a>");
                }
                _ => {}
            },
            Event::Text(text) => html.push_str(&escape_html(&text)),
            Event::Code(code) => {
                html.push_str("<code>");
                html.push_str(&escape_html(&code));
                html.push_str("</code>");
            }
            Event::Html(raw) | Event::InlineHtml(raw) => {
                html.push_str(&escape_html(&raw));
            }
            Event::SoftBreak | Event::HardBreak => html.push(' '),
            _ => {}
        }
    }
    html
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoomSection {
    pub id: u64,
    pub name: String,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TicketNote {
    pub at_ms: i64,
    pub column: Column,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TicketEvent {
    pub seq: u64,
    pub at_ms: i64,
    pub kind: String,
    pub from_column: Option<Column>,
    pub to_column: Option<Column>,
    pub summary: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Ticket {
    pub id: String,
    pub room_id: u64,
    pub room_name: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub description_html: String,
    pub column: Column,
    pub investigation_only: bool,
    pub mr_url: Option<String>,
    pub author_model: Option<String>,
    pub reviewer_model: Option<String>,
    pub notes: Vec<TicketNote>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub history: Vec<TicketEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionEvent {
    pub seq: u64,
    pub at_ms: i64,
    pub kind: String,
    pub room_id: Option<u64>,
    pub room_name: Option<String>,
    pub ticket_id: Option<String>,
    pub ticket_title: Option<String>,
    pub from_column: Option<Column>,
    pub to_column: Option<Column>,
    pub summary: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardState {
    pub version: u32,
    pub next_ticket_num: u64,
    pub next_event_seq: u64,
    pub rooms: Vec<RoomSection>,
    pub tickets: Vec<Ticket>,
    pub session_history: Vec<SessionEvent>,
}

impl Default for BoardState {
    fn default() -> Self {
        Self {
            version: 1,
            next_ticket_num: 1,
            next_event_seq: 1,
            rooms: Vec::new(),
            tickets: Vec::new(),
            session_history: Vec::new(),
        }
    }
}

pub struct NewTicketInput {
    pub room: String,
    pub title: String,
    pub description: Option<String>,
    pub column: Column,
    pub mr_url: Option<String>,
    pub author_model: Option<String>,
    pub reviewer_model: Option<String>,
    pub investigation_only: bool,
    pub note: Option<String>,
}

pub struct UpdateTicketInput {
    pub id: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub mr_url: Option<String>,
    pub author_model: Option<String>,
    pub reviewer_model: Option<String>,
    pub note: Option<String>,
}

fn nonempty_model(model: String) -> Option<String> {
    let model = model.trim();
    (!model.is_empty()).then(|| model.to_string())
}

impl BoardState {
    fn alloc_seq(&mut self) -> u64 {
        let s = self.next_event_seq;
        self.next_event_seq += 1;
        s
    }

    pub fn add_room(&mut self, id: u64, name: String, at_ms: i64) -> Result<RoomSection> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(invalid("room name must not be empty"));
        }
        if let Some(existing) = self.rooms.iter_mut().find(|r| r.id == id) {
            existing.name = name.clone();
            for t in self.tickets.iter_mut().filter(|t| t.room_id == id) {
                t.room_name = name.clone();
            }
            let seq = self.alloc_seq();
            self.session_history.push(SessionEvent {
                seq,
                at_ms,
                kind: "room_updated".into(),
                room_id: Some(id),
                room_name: Some(name.clone()),
                ticket_id: None,
                ticket_title: None,
                from_column: None,
                to_column: None,
                summary: format!("Updated section #{id} ({name})"),
                note: None,
            });
            return Ok(self.rooms.iter().find(|r| r.id == id).unwrap().clone());
        }
        if self
            .rooms
            .iter()
            .any(|r| r.name.eq_ignore_ascii_case(&name))
        {
            return Err(invalid(format!(
                "room name '{name}' is already registered under another id"
            )));
        }
        let section = RoomSection {
            id,
            name: name.clone(),
            created_at_ms: at_ms,
        };
        self.rooms.push(section.clone());
        self.rooms.sort_by_key(|r| r.id);
        let seq = self.alloc_seq();
        self.session_history.push(SessionEvent {
            seq,
            at_ms,
            kind: "room_added".into(),
            room_id: Some(id),
            room_name: Some(name.clone()),
            ticket_id: None,
            ticket_title: None,
            from_column: None,
            to_column: None,
            summary: format!("Registered section #{id} ({name})"),
            note: None,
        });
        Ok(section)
    }

    pub fn resolve_room(&self, name_or_id: &str) -> Result<&RoomSection> {
        let q = name_or_id.trim().trim_start_matches('#');
        if let Ok(id) = q.parse::<u64>() {
            if let Some(r) = self.rooms.iter().find(|r| r.id == id) {
                return Ok(r);
            }
        }
        if let Some(r) = self.rooms.iter().find(|r| r.name.eq_ignore_ascii_case(q)) {
            return Ok(r);
        }
        Err(not_found(format!(
            "room '{name_or_id}' not found; register it first with 'agile-board room add --id <ID> --name <NAME>'"
        )))
    }

    pub fn add_ticket(&mut self, input: NewTicketInput, at_ms: i64) -> Result<Ticket> {
        let title = input.title.trim().to_string();
        if title.is_empty() {
            return Err(invalid("ticket title must not be empty"));
        }
        if input.column == Column::Done && !input.investigation_only {
            return Err(transition_denied(
                "cannot create a non-investigation-only ticket directly in 'done'; start in todo, investigation, implementation, or blocked",
            ));
        }
        let room = self.resolve_room(&input.room)?.clone();
        let ticket_id = format!("T-{}", self.next_ticket_num);
        self.next_ticket_num += 1;

        let mut notes = Vec::new();
        let note_clean = input
            .note
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if let Some(ref text) = note_clean {
            notes.push(TicketNote {
                at_ms,
                column: input.column,
                text: text.clone(),
            });
        }
        let description = input
            .description
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| note_clean.clone())
            .unwrap_or_default();
        let description_html = render_sanitized_markdown(&description);
        let mr_url = input
            .mr_url
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let seq = self.alloc_seq();
        let summary = format!(
            "Created {ticket_id} in {} ({})",
            room.name,
            input.column.as_str()
        );
        let ev = TicketEvent {
            seq,
            at_ms,
            kind: "ticket_created".into(),
            from_column: None,
            to_column: Some(input.column),
            summary: summary.clone(),
            note: note_clean.clone(),
        };
        let ticket = Ticket {
            id: ticket_id.clone(),
            room_id: room.id,
            room_name: room.name.clone(),
            title: title.clone(),
            description,
            description_html,
            column: input.column,
            investigation_only: input.investigation_only,
            mr_url,
            author_model: input.author_model.and_then(nonempty_model),
            reviewer_model: input.reviewer_model.and_then(nonempty_model),
            notes,
            created_at_ms: at_ms,
            updated_at_ms: at_ms,
            history: vec![ev],
        };
        self.tickets.push(ticket.clone());
        self.session_history.push(SessionEvent {
            seq,
            at_ms,
            kind: "ticket_created".into(),
            room_id: Some(room.id),
            room_name: Some(room.name),
            ticket_id: Some(ticket_id),
            ticket_title: Some(title),
            from_column: None,
            to_column: Some(input.column),
            summary,
            note: note_clean,
        });
        Ok(ticket)
    }

    pub fn move_ticket(
        &mut self,
        id: &str,
        to: Column,
        note: Option<String>,
        at_ms: i64,
    ) -> Result<Ticket> {
        let q = id.trim();
        let idx = self
            .tickets
            .iter()
            .position(|t| t.id.eq_ignore_ascii_case(q))
            .ok_or_else(|| not_found(format!("ticket '{q}' not found")))?;
        let current = self.tickets[idx].column;
        let inv_only = self.tickets[idx].investigation_only;
        let canonical_id = self.tickets[idx].id.clone();
        current.validate_transition(to, inv_only, &canonical_id)?;

        let seq = self.alloc_seq();
        let note_clean = note.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());

        let t = &mut self.tickets[idx];
        t.column = to;
        t.updated_at_ms = at_ms;
        if let Some(ref text) = note_clean {
            t.notes.push(TicketNote {
                at_ms,
                column: to,
                text: text.clone(),
            });
            if t.description.is_empty() {
                t.description = text.clone();
                t.description_html = render_sanitized_markdown(&t.description);
            }
        }
        let summary = format!(
            "Moved {} from {} to {}",
            t.id,
            current.as_str(),
            to.as_str()
        );
        t.history.push(TicketEvent {
            seq,
            at_ms,
            kind: "ticket_moved".into(),
            from_column: Some(current),
            to_column: Some(to),
            summary: summary.clone(),
            note: note_clean.clone(),
        });
        let updated = t.clone();
        self.session_history.push(SessionEvent {
            seq,
            at_ms,
            kind: "ticket_moved".into(),
            room_id: Some(updated.room_id),
            room_name: Some(updated.room_name.clone()),
            ticket_id: Some(updated.id.clone()),
            ticket_title: Some(updated.title.clone()),
            from_column: Some(current),
            to_column: Some(to),
            summary,
            note: note_clean,
        });
        Ok(updated)
    }

    pub fn update_ticket(&mut self, input: UpdateTicketInput, at_ms: i64) -> Result<Ticket> {
        let q = input.id.trim();
        let idx = self
            .tickets
            .iter()
            .position(|t| t.id.eq_ignore_ascii_case(q))
            .ok_or_else(|| not_found(format!("ticket '{q}' not found")))?;

        let mut changes = Vec::new();
        let note_clean = input
            .note
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        if let Some(new_title) = input.title {
            let trimmed = new_title.trim().to_string();
            if trimmed.is_empty() {
                return Err(invalid("ticket title must not be empty"));
            }
            self.tickets[idx].title = trimmed;
            changes.push("title");
        }
        if let Some(desc) = input.description {
            let trimmed = desc.trim().to_string();
            self.tickets[idx].description_html = render_sanitized_markdown(&trimmed);
            self.tickets[idx].description = trimmed;
            changes.push("description");
        }
        if let Some(mr) = input.mr_url {
            let trimmed = mr.trim().to_string();
            self.tickets[idx].mr_url = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            };
            changes.push("mr_url");
        }
        if let Some(model) = input.author_model {
            self.tickets[idx].author_model = nonempty_model(model);
            changes.push("author_model");
        }
        if let Some(model) = input.reviewer_model {
            self.tickets[idx].reviewer_model = nonempty_model(model);
            changes.push("reviewer_model");
        }
        if let Some(ref text) = note_clean {
            let col = self.tickets[idx].column;
            self.tickets[idx].notes.push(TicketNote {
                at_ms,
                column: col,
                text: text.clone(),
            });
            changes.push("note");
        }
        if changes.is_empty() {
            return Err(invalid(
                "ticket update requires at least one of --title, --description, --mr, --author-model, --reviewer-model, or --note",
            ));
        }

        let seq = self.alloc_seq();
        let t = &mut self.tickets[idx];
        t.updated_at_ms = at_ms;
        let summary = format!("Updated {} ({})", t.id, changes.join(", "));
        t.history.push(TicketEvent {
            seq,
            at_ms,
            kind: "ticket_updated".into(),
            from_column: Some(t.column),
            to_column: Some(t.column),
            summary: summary.clone(),
            note: note_clean.clone(),
        });
        let updated = t.clone();
        self.session_history.push(SessionEvent {
            seq,
            at_ms,
            kind: "ticket_updated".into(),
            room_id: Some(updated.room_id),
            room_name: Some(updated.room_name.clone()),
            ticket_id: Some(updated.id.clone()),
            ticket_title: Some(updated.title.clone()),
            from_column: Some(updated.column),
            to_column: Some(updated.column),
            summary,
            note: note_clean,
        });
        Ok(updated)
    }

    pub fn clear_session_history(&mut self) -> (usize, usize) {
        let cleared_tickets = self.tickets.len();
        let cleared_events = self.session_history.len();
        self.tickets.clear();
        self.session_history.clear();
        self.next_ticket_num = 1;
        self.next_event_seq = 1;
        (cleared_tickets, cleared_events)
    }
}
