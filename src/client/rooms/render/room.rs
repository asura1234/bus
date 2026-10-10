//! Room history, composer, notes, search and recipient views.
use super::super::{
    chat_search::ChatSearch,
    history::{Line, Tone},
    recipients::Layout as RecipientLayout,
    thumbnails::Placement,
    BusUi, ComposerSize, LocalRoom,
};
use super::dialogs::{search_panel, SEARCH_BOX_HEIGHT};
use super::sidebar::{message_name_color, status_name};
use super::text::{display, provider, wrap};
use super::{Action, Hit, View, ACCENT};
use crate::messaging::model::{Room, RoomId, RoomKind};
use ratatui::{
    layout::Rect,
    style::{Color, Style},
};
use std::{path::PathBuf, sync::Arc};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Background of chat-search matches, and of the selected match.
const SEARCH_MATCH: Color = Color::Rgb(150, 125, 60);
const SEARCH_CURRENT: Color = Color::Rgb(240, 190, 70);

/// Geometry reserved before painting the room's notes, history and draft.
#[derive(Clone, Copy)]
struct RoomLayout {
    x: u16,
    width: u16,
    composer_y: u16,
    bar_height: u16,
    text_height: u16,
    max_height: u16,
}

impl BusUi {
    pub(super) fn room_view(&mut self, view: &mut View, main: Rect) {
        let Some(room) = self.room.and_then(|id| self.snapshot.state.room(id)) else {
            view.lines(
                Rect::new(main.x + 2, 2, main.width.saturating_sub(4), 3),
                "No rooms. Click ROOMS + or press Ctrl+Shift+R to add one.",
                None,
                true,
            );
            return;
        };
        let Some(local) = self.locals.get(&room.id) else {
            return;
        };
        let width = main.width.saturating_sub(4);
        let status = self.room_status(room.id, width);
        let recipients = super::super::recipients::layout(
            local.recipients.iter().filter_map(|id| {
                self.snapshot
                    .state
                    .agent(*id)
                    .map(|agent| (agent.id, agent.name.as_str()))
            }),
            width,
        );
        let chat_search = self.chat_search.clone();
        let layout = self.composer_layout(view, main, local, &recipients, &status);
        self.recipient_scroll = self.recipient_scroll.min(view.recipient_max_scroll);
        let history_y = self.room_header(view, main, room, local, layout);
        self.notes_view(view, local);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis() as u64);
        let visible_anchor = (!self.history_follow_tail)
            .then(|| self.history.anchor_at(self.main_scroll))
            .flatten();
        self.history.lines(
            &self.snapshot.state,
            room,
            width,
            self.snapshot.revision,
            now,
            &mut self.thumbnails,
        );
        if let Some(anchor) = visible_anchor {
            if let Some(index) = self.history.index_of(anchor) {
                self.main_scroll = index;
            }
        }
        let content = self.history.cached();
        view.history = Rect::new(
            main.x,
            history_y,
            main.width,
            view.composer_box.y.saturating_sub(history_y),
        );
        view.history_max_scroll = content
            .len()
            .saturating_sub(usize::from(view.history.height));
        view.search_matches = chat_search
            .as_ref()
            .map(|search| super::super::chat_search::find_matches(content, &search.query))
            .unwrap_or_default();
        update_chat_search(
            &mut self.chat_search,
            view,
            layout.x,
            width,
            &mut self.main_scroll,
            &mut self.history_follow_tail,
        );
        self.main_scroll = if self.history_follow_tail {
            view.history_max_scroll
        } else {
            self.main_scroll.min(view.history_max_scroll)
        };
        view.history_text = Rect::new(layout.x, history_y, width, view.history.height);
        self.history_rows(view, content);
        view.lines(
            Rect::new(layout.x, main.bottom().saturating_sub(1), width, 1),
            &status,
            None,
            true,
        );
        self.recipient_bar_view(view, local, &recipients, layout);
        self.composer_editor_view(view, local, layout);
        let files = self.draft_files(room);
        self.file_scroll = self.file_scroll.min(files.len().saturating_sub(1));
        file_rows(view, &files, self.file_scroll, layout);
        self.file_detail_view(view, layout);
        self.recipient_menu_view(view, room, local, layout);
    }

    // The caller resolved its selected room before rendering; reuse that ID
    // rather than looking it up again and unwrapping the selection.
    fn room_status(&self, room: RoomId, width: u16) -> String {
        let queued: usize = self
            .snapshot
            .state
            .agents()
            .filter(|a| a.room_id == room)
            .map(|a| self.snapshot.state.queued_requests(a.id).len())
            .sum();
        let notice = self.toast_text_at(std::time::Instant::now(), width);
        let search_status = self
            .history_search
            .as_ref()
            .map(|search| (search.query.clone(), search.selected));
        let search_status = search_status.map(|(query, selected)| {
            let total = self.filtered_history(room, &query).len().max(1);
            (query, selected, total)
        });
        notice
            .or_else(|| {
                search_status.as_ref().map(|(query, selected, total)| {
                    format!("search {query}  {}/{total}", selected + 1)
                })
            })
            .unwrap_or_else(|| {
                // A pending send shows nothing: it completes in milliseconds,
                // so a status row only flashes; the client log records it.
                if queued > 0 {
                    format!("{queued} queued requests")
                } else {
                    String::new()
                }
            })
    }

    fn composer_layout(
        &self,
        view: &mut View,
        main: Rect,
        local: &LocalRoom,
        recipients: &RecipientLayout,
        status: &str,
    ) -> RoomLayout {
        let x = main.x + 2;
        let width = main.width.saturating_sub(4);
        let bottom = main.bottom();
        let composer_bottom = bottom.saturating_sub(u16::from(!status.is_empty()));
        // Preserve the eight-row room header/notes, at least three history
        // rows, a three-row draft, and its four chrome rows. The logical chip
        // list still wraps without a row limit and scrolls in this viewport.
        let bar_budget = composer_bottom.saturating_sub(18);
        let bar_height = if recipients.chips.is_empty() {
            1
        } else {
            recipients.height.min((bar_budget / 3 * 3).max(3))
        };
        view.recipient_max_scroll = recipients.height.saturating_sub(bar_height);
        // One active draft per frame, independent of agent/pane cardinality.
        // Keep the box flush with the bottom unless a real notice needs a row.
        let max_height = composer_bottom.saturating_sub(bar_height + 4);
        let text_height = if self.notes_focus {
            // In short terminals, notes need a visible row and caret before
            // reserving space for the temporarily inactive draft editor.
            3.min(composer_bottom.saturating_sub(bar_height + 9))
        } else {
            match local.composer_size {
                ComposerSize::Auto => wrap(&local.text.text, width)
                    .len()
                    .max(3)
                    .min(usize::from(max_height)) as u16,
                ComposerSize::Full => max_height,
                ComposerSize::Compact => 3,
            }
        }
        .min(max_height);
        let composer_y = composer_bottom.saturating_sub(text_height + bar_height + 3);
        view.composer_box = Rect::new(
            main.x + 1,
            composer_y.saturating_sub(1),
            main.width.saturating_sub(2),
            text_height + bar_height + 4,
        )
        .intersection(main);
        view.composer_divider = Rect::new(
            main.x + 1,
            composer_y + bar_height,
            main.width.saturating_sub(2),
            1,
        )
        .intersection(main);
        view.recipient_bar = Rect::new(x, composer_y, width, bar_height);
        RoomLayout {
            x,
            width,
            composer_y,
            bar_height,
            text_height,
            max_height,
        }
    }

    fn room_header(
        &self,
        view: &mut View,
        main: Rect,
        room: &Room,
        local: &LocalRoom,
        layout: RoomLayout,
    ) -> u16 {
        let RoomLayout {
            x,
            width,
            composer_y,
            ..
        } = layout;
        if composer_y > 2 {
            view.row(
                Rect::new(x, 1, width, 1),
                format!("# {}", room.name),
                None,
                false,
                false,
            );
        }
        // The notes box grows with its text up to a quarter of the window,
        // keeps one row for the F3 prompt, and scrolls beyond that. History
        // takes the rows below it. MASTER has no notes (the human talks to
        // orchestrators there; status boards live in work rooms), so its
        // history starts right under the room name.
        let has_notes = room.kind != RoomKind::Master;
        let note_lines = wrap(&local.notes.text, width).len().max(1);
        let note_height = if has_notes {
            (note_lines.min(usize::from((main.height / 4).max(1))) as u16)
                .min(composer_y.saturating_sub(5))
        } else {
            0
        };
        if note_height > 0 {
            view.notes_box =
                Rect::new(main.x + 1, 2, main.width.saturating_sub(2), note_height + 2);
        }
        // The expanded draft may hide the top section. Never paint a history
        // separator over its editor or reserve rows from full-screen editing.
        let mut history_y = if has_notes { 5 + note_height.max(1) } else { 3 };
        if history_y - 1 < view.composer_box.y {
            view.history_divider =
                Rect::new(main.x + 1, history_y - 1, main.width.saturating_sub(2), 1);
        }
        // The open search takes the top of the history pane, full width, and
        // the history shrinks by its height until it closes.
        if self.chat_search.is_some() && history_y + SEARCH_BOX_HEIGHT < view.composer_box.y {
            view.search_box = Rect::new(
                main.x + 1,
                history_y,
                main.width.saturating_sub(2),
                SEARCH_BOX_HEIGHT,
            );
            history_y += SEARCH_BOX_HEIGHT;
        }
        view.notes = Rect::new(x, 3, width, note_height);
        history_y
    }

    fn notes_view(&self, view: &mut View, local: &LocalRoom) {
        let note_height = view.notes.height;
        (view.notes_scroll, view.notes_rows) = view.editor_scrolled(
            view.notes,
            &local.notes,
            Some(Action::Notes),
            self.notes_focus,
            // Read unfocused notes from the top unless the wheel moved them.
            local.notes_scroll.or((!self.notes_focus).then_some(0)),
            self.view.notes_scroll,
        );
        // Mark hidden notes on the box's top and bottom borders.
        let notes_box = view.notes_box;
        if note_height > 0 && notes_box.width > 12 {
            let more_x = notes_box.right().saturating_sub(9);
            if view.notes_scroll > 0 {
                view.row(
                    Rect::new(more_x, notes_box.y, 7, 1),
                    "↑ more",
                    None,
                    false,
                    true,
                );
            }
            if view.notes_scroll + usize::from(note_height) < view.notes_rows {
                view.row(
                    Rect::new(more_x, notes_box.bottom() - 1, 7, 1),
                    "↓ more",
                    None,
                    false,
                    true,
                );
            }
        }
        if note_height > 0 && local.notes.text.is_empty() {
            view.row(
                Rect::new(view.notes.x, 3, view.notes.width, 1),
                "Add notes… (F3)",
                Some(Action::Notes),
                false,
                true,
            );
        }
    }

    fn history_rows(&self, view: &mut View, content: &[Line]) {
        let Rect {
            x,
            y: history_y,
            width,
            ..
        } = view.history_text;
        let current_match = self
            .chat_search
            .as_ref()
            .and_then(|search| view.search_matches.get(search.current))
            .cloned();
        let selection =
            super::super::selection::normalize_history_selection(content, self.history_selection);
        for (index, line) in content
            .iter()
            .skip(self.main_scroll)
            .take(usize::from(view.history.height))
            .enumerate()
        {
            let rect = Rect::new(x, history_y + index as u16, width, 1);
            view.row(
                rect,
                &line.text,
                line.action.clone(),
                false,
                matches!(line.tone, Tone::Muted),
            );
            if let Some(thumbnail) = line.thumbnail.as_ref().filter(|thumbnail| {
                // Partly scrolled thumbnails keep their blank rows and name.
                thumbnail.row == 0
                    && index + usize::from(thumbnail.rows) <= usize::from(view.history.height)
            }) {
                view.thumbnails.push(Placement {
                    path: Arc::clone(&thumbnail.path),
                    x,
                    y: rect.y,
                    cols: thumbnail.cols,
                    rows: thumbnail.rows,
                });
            }
            let line_index = self.main_scroll + index;
            if let Some((start, end)) =
                selection.filter(|(start, end)| (start.line..=end.line).contains(&line_index))
            {
                let from = if line_index == start.line {
                    start.offset
                } else {
                    0
                };
                let to = if line_index == end.line {
                    end.offset
                } else {
                    usize::MAX
                };
                let newline = content
                    .get(line_index + 1)
                    .is_some_and(|next| !next.continued);
                view.select(rect, &line.text, 0, &(from..to), newline);
            }
            self.history_row_styles(view, line, rect);
            highlight_history_row(view, line, line_index, rect, current_match.as_ref());
        }
    }

    fn history_row_styles(&self, view: &mut View, line: &Line, rect: Rect) {
        let width = rect.width;
        let mut column = 0u16;
        for (text, tone) in &line.spans {
            let span_width = text.width() as u16;
            let color = match tone {
                Tone::You => Some(ACCENT),
                Tone::Agent(id) => self.snapshot.state.agent(*id).map(|agent| {
                    message_name_color(&self.snapshot.state, self.room, agent, &self.settings)
                }),
                _ => None,
            };
            if let Some(color) = color {
                let span_rect = Rect::new(
                    rect.x + column,
                    rect.y,
                    span_width.min(width.saturating_sub(column)),
                    1,
                );
                view.row(span_rect, text, None, false, false);
                view.color_last_row(span_rect, color);
            }
            column = column.saturating_add(span_width);
        }
        let mut column = 0u16;
        for (text, style) in &line.styles {
            let span_width = text.width() as u16;
            let span_rect = Rect::new(
                rect.x + column,
                rect.y,
                span_width.min(width.saturating_sub(column)),
                1,
            );
            view.row(span_rect, text, None, false, false);
            view.style_last_row(span_rect, *style);
            column = column.saturating_add(span_width);
        }
    }

    fn recipient_bar_view(
        &self,
        view: &mut View,
        local: &LocalRoom,
        recipients: &RecipientLayout,
        layout: RoomLayout,
    ) {
        let RoomLayout {
            x,
            width,
            composer_y,
            bar_height,
            ..
        } = layout;
        let controls_y = composer_y + u16::from(bar_height > 1);
        view.row(
            Rect::new(x, controls_y, 3, 1),
            "+",
            Some(Action::Files),
            false,
            false,
        );
        view.row(
            Rect::new(x + 3, controls_y, width.saturating_sub(3), 1),
            if local.recipients.is_empty() {
                "@  Choose agents"
            } else {
                "@"
            },
            Some(Action::Recipients),
            false,
            true,
        );
        for chip in &recipients.chips {
            if chip.rect.y < self.recipient_scroll
                || chip.rect.bottom() > self.recipient_scroll + bar_height
            {
                continue;
            }
            let rect = Rect::new(
                x + chip.rect.x,
                composer_y + chip.rect.y - self.recipient_scroll,
                chip.rect.width,
                3,
            );
            let color = self
                .snapshot
                .state
                .agent(chip.agent)
                .map_or(ACCENT, |agent| {
                    message_name_color(&self.snapshot.state, self.room, agent, &self.settings)
                });
            view.recipient_chips.push((rect, color));
            let label_rect = Rect::new(rect.x + 2, rect.y + 1, rect.width.saturating_sub(4), 1);
            view.row(label_rect, &chip.label, None, false, false);
            view.color_last_row(label_rect, color);
            view.hits.push(Hit {
                rect,
                action: Action::Recipients,
            });
        }
    }

    fn composer_editor_view(&self, view: &mut View, local: &LocalRoom, layout: RoomLayout) {
        let RoomLayout {
            x,
            width,
            composer_y,
            bar_height,
            text_height,
            max_height,
        } = layout;
        let editor_y = composer_y + bar_height + 1;
        view.composer = Rect::new(x, editor_y, width, text_height);
        view.composer_max_height = max_height;
        (view.composer_scroll, view.composer_rows) = view.editor_scrolled(
            view.composer,
            &local.text,
            Some(Action::Composer),
            !self.notes_focus
                && !self.recipient_menu
                && self.rename.is_none()
                && self.chat_search.is_none(),
            local.composer_scroll,
            self.view.composer_scroll,
        );
        if local.text.text.is_empty() {
            view.row(
                Rect::new(x, editor_y, width, 1),
                "Message selected agents…",
                Some(Action::Composer),
                false,
                true,
            );
        }
    }

    fn draft_files(&self, room: &Room) -> Vec<(PathBuf, bool)> {
        let mut files: Vec<_> = room
            .draft
            .files
            .iter()
            .cloned()
            .map(|path| (path, false))
            .collect();
        for pending in &self.failed {
            if let crate::messaging::coordinator::BusCommand::AttachFile(id, path) =
                &pending.command
            {
                if *id != room.id {
                    continue;
                }
                files.push((PathBuf::from(path), true));
            }
        }
        files
    }

    fn file_detail_view(&self, view: &mut View, layout: RoomLayout) {
        let RoomLayout {
            x,
            width,
            composer_y,
            ..
        } = layout;
        if let Some(path) = &self.detail_path {
            let lines = wrap(path, width);
            let fits_above = lines.len() <= usize::from(composer_y.saturating_sub(7));
            let popup_bottom = if fits_above {
                composer_y.saturating_sub(1)
            } else {
                view.composer.bottom()
            };
            let height = lines.len().min(usize::from(popup_bottom.saturating_sub(3))) as u16;
            let popup = Rect::new(x, popup_bottom.saturating_sub(height), width, height);
            for (index, line) in lines.into_iter().take(usize::from(height)).enumerate() {
                view.overlay_row(
                    Rect::new(x, popup.y + index as u16, width, 1),
                    line,
                    None,
                    false,
                );
            }
            if view
                .cursor
                .as_ref()
                .is_some_and(|cursor| popup.contains((cursor.x, cursor.y).into()))
            {
                view.cursor = None;
            }
            view.selection.retain(|rect| !rect.intersects(popup));
            view.thumbnails
                .retain(|thumbnail| !thumbnail.rect().intersects(popup));
        }
    }

    fn recipient_menu_view(
        &self,
        view: &mut View,
        room: &Room,
        local: &LocalRoom,
        layout: RoomLayout,
    ) {
        let RoomLayout {
            x,
            width,
            composer_y,
            text_height,
            ..
        } = layout;
        if self.recipient_menu {
            let agents: Vec<_> = self
                .snapshot
                .state
                .agents()
                .filter(|a| a.room_id == room.id)
                .collect();
            let available_above = composer_y.saturating_sub(2);
            // At full height the picker overlays the draft, not an empty area
            // above it. Its later hit targets win over the editor beneath.
            let (y, height) = if available_above >= (agents.len() + 1).min(3) as u16 {
                let height = (agents.len() + 1).min(usize::from(available_above)) as u16;
                (composer_y - 1 - height, height)
            } else {
                (
                    view.composer.y,
                    (agents.len() + 1).min(usize::from(text_height)) as u16,
                )
            };
            let start = self
                .recipient_index
                .saturating_sub(height.saturating_sub(1) as usize);
            let all = !agents.is_empty() && agents.iter().all(|a| local.recipients.contains(&a.id));
            let entries = std::iter::once((format!("[{}] All", if all { "x" } else { " " }), None))
                .chain(agents.iter().map(|a| {
                    (
                        format!(
                            "[{}] {}  {}  {}",
                            if local.recipients.contains(&a.id) {
                                "x"
                            } else {
                                " "
                            },
                            a.name,
                            provider(a.provider),
                            status_name(a.shown_status())
                        ),
                        Some(a.id),
                    )
                }));
            for (index, (label, id)) in entries.enumerate().skip(start).take(height as usize) {
                view.overlay_row(
                    Rect::new(x, y + (index - start) as u16, width, 1),
                    label,
                    Some(Action::Recipient(id)),
                    index == self.recipient_index,
                );
            }
            view.cursor = None;
            let menu = Rect::new(x, y, width, height);
            view.selection.retain(|rect| !rect.intersects(menu));
            // Kitty images draw over text, so they would hide the choices.
            view.thumbnails
                .retain(|thumbnail| !thumbnail.rect().intersects(menu));
        }
    }
}

fn update_chat_search(
    chat_search: &mut Option<ChatSearch>,
    view: &mut View,
    x: u16,
    width: u16,
    scroll: &mut usize,
    follow_tail: &mut bool,
) {
    if let Some(search) = chat_search.as_mut() {
        let count = view.search_matches.len();
        if search.jump_to_newest {
            // A new query starts from the newest match, nearest the bottom.
            search.jump_to_newest = false;
            search.current = count.saturating_sub(1);
            if let Some((row, _)) = view.search_matches.last() {
                *follow_tail = false;
                *scroll = row.saturating_sub(usize::from(view.history.height) / 3);
            }
        }
        search.current = search.current.min(count.saturating_sub(1));
        search_panel(view, search, x, width, count);
    }
}

fn highlight_history_row(
    view: &mut View,
    line: &Line,
    line_index: usize,
    rect: Rect,
    current_match: Option<&(usize, std::ops::Range<usize>)>,
) {
    let width = rect.width;
    // Search matches are painted last, over any span colors.
    for (row, range) in view
        .search_matches
        .iter()
        .filter(|(row, _)| *row == line_index)
        .cloned()
        .collect::<Vec<_>>()
    {
        let Some(found) = line.text.get(range.clone()) else {
            continue;
        };
        let column = line.text[..range.start].width() as u16;
        let found_width = found.width() as u16;
        let span_rect = Rect::new(
            rect.x + column,
            rect.y,
            found_width.min(width.saturating_sub(column)),
            1,
        );
        let current = current_match == Some(&(row, range));
        view.row(span_rect, found, None, false, false);
        view.style_last_row(
            span_rect,
            Style::default().fg(Color::Rgb(24, 24, 28)).bg(if current {
                SEARCH_CURRENT
            } else {
                SEARCH_MATCH
            }),
        );
    }
}

fn file_rows(view: &mut View, files: &[(PathBuf, bool)], scroll: usize, layout: RoomLayout) {
    let RoomLayout { x, width, .. } = layout;
    let file_y = layout.composer_y + layout.bar_height + 1 + layout.text_height;
    if !files.is_empty() {
        view.files = Rect::new(x, file_y, width, 1);
    }
    // Reserve overflow controls before placing chips, so even one very long
    // filename cannot consume the route to later successful/failed files.
    let controls = files.len() > 1;
    let mut chip_x = x + if controls { 3 } else { 0 };
    let chip_right = x + width.saturating_sub(if controls { 3 } else { 0 });
    let mut shown = 0;
    for (path, failed) in files.iter().skip(scroll) {
        let suffix = if *failed { " ! ×]" } else { " ×]" };
        let available = chip_right.saturating_sub(chip_x);
        if available < suffix.len() as u16 + 2 {
            break;
        }
        let name = display(&path.file_name().unwrap_or_default().to_string_lossy());
        let budget = available.saturating_sub(if *failed { 7 } else { 5 });
        let mut used = 0;
        let clipped: String = name
            .chars()
            .take_while(|c| {
                used += c.width().unwrap_or(0) as u16;
                used <= budget
            })
            .collect();
        let label = format!("[{clipped}{suffix} ");
        let size = label.width() as u16;
        view.row(
            Rect::new(chip_x, file_y, size, 1),
            label,
            Some(Action::RemoveFile(path.clone())),
            false,
            true,
        );
        chip_x += size;
        shown += 1;
    }
    if controls && scroll > 0 {
        view.row(
            Rect::new(x, file_y, 2, 1),
            "<",
            Some(Action::ScrollFiles(false)),
            false,
            true,
        );
    }
    if controls && scroll + shown < files.len() {
        view.row(
            Rect::new(x + width.saturating_sub(2), file_y, 2, 1),
            ">",
            Some(Action::ScrollFiles(true)),
            false,
            true,
        );
    }
}
