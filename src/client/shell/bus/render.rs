use super::*;
use super::{
    deletion::DeleteTarget,
    editor::Editor,
    forms::{Form, RenameTarget},
};
use crate::bus::model::*;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Widget};
use ratatui::{buffer::Buffer, layout::Rect};
use std::ops::Range;
use unicode_width::UnicodeWidthChar;

/// Background of mouse-selected text.
const SELECTION: Color = Color::Rgb(44, 88, 56);
const ACCENT: Color = {
    let [r, g, b] = crate::bus::colors::YOU_COLOR;
    Color::Rgb(r, g, b)
};
const WORKING_MID: Color = Color::Rgb(68, 190, 84);
const WORKING_DIM: Color = Color::Rgb(36, 112, 52);
const BLOCKED_BRIGHT: Color = Color::Rgb(255, 92, 102);
const BLOCKED_MID: Color = Color::Rgb(205, 64, 72);
const BLOCKED_DIM: Color = Color::Rgb(128, 44, 52);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Delete(DeleteTarget),
    CancelDelete,
    ConfirmDelete,
    Room(crate::bus::model::RoomId),
    Agent(crate::bus::model::AgentId),
    NewRoom,
    NewAgent,
    ScrollFiles(bool),
    Notes,
    Composer,
    Recipients,
    Recipient(Option<crate::bus::model::AgentId>),
    Files,
    RemoveFile(std::path::PathBuf),
    FileDetail(std::path::PathBuf),
    Details(crate::bus::model::AgentId),
    Quote(crate::bus::model::RequestId),
    Field(usize),
    Provider(Provider),
    Suggestion(usize),
    Cancel,
    Add,
}
#[derive(Clone, Debug)]
pub(super) struct Hit {
    pub rect: Rect,
    pub action: Action,
}
#[derive(Clone)]
struct Row {
    x: u16,
    y: u16,
    width: u16,
    text: String,
    selected: bool,
    muted: bool,
    color: Option<Color>,
    character_colors: Option<Vec<Color>>,
}
#[derive(Default)]
pub(super) struct View {
    pub hits: Vec<Hit>,
    pub sidebar: Rect,
    pub sidebar_body: Rect,
    pub sidebar_max_scroll: usize,
    sidebar_divider: Rect,
    pub history: Rect,
    pub history_max_scroll: usize,
    pub recipient_bar: Rect,
    pub recipient_max_scroll: u16,
    recipient_chips: Vec<(Rect, Color)>,
    pub files: Rect,
    pub composer: Rect,
    pub composer_max_height: u16,
    pub composer_scroll: usize,
    pub composer_rows: usize,
    composer_box: Rect,
    composer_divider: Rect,
    notes_box: Rect,
    history_divider: Rect,
    dialog: Rect,
    dialog_rows_start: usize,
    pub help: Rect,
    pub help_scroll: usize,
    pub help_max_scroll: usize,
    rows: Vec<Row>,
    pub cursor: Option<crate::protocol::CursorState>,
    pub notes: Rect,
    pub notes_scroll: usize,
    /// Text columns of the visible history rows.
    pub history_text: Rect,
    /// Selected cells, painted after the rows they cover.
    selection: Vec<Rect>,
}
impl View {
    fn overlay_row(&mut self, rect: Rect, text: String, action: Option<Action>, selected: bool) {
        // Overlay labels must erase the draft/reply suffix beneath them too.
        let text = display(&text);
        let padding = usize::from(rect.width)
            .saturating_sub(unicode_width::UnicodeWidthStr::width(text.as_str()));
        self.row(
            rect,
            format!("{text}{}", " ".repeat(padding)),
            action,
            selected,
            false,
        );
    }
    fn row(
        &mut self,
        rect: Rect,
        text: impl Into<String>,
        action: Option<Action>,
        selected: bool,
        muted: bool,
    ) {
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        self.rows.push(Row {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            text: text.into(),
            selected,
            muted,
            color: None,
            character_colors: None,
        });
        if let Some(action) = action {
            self.hits.push(Hit { rect, action });
        }
    }
    fn color_last_row(&mut self, rect: Rect, color: Color) {
        if rect.width > 0 && rect.height > 0 {
            if let Some(row) = self.rows.last_mut() {
                row.color = Some(color);
            }
        }
    }
    fn color_last_row_characters(&mut self, rect: Rect, colors: Vec<Color>) {
        if rect.width > 0 && rect.height > 0 {
            if let Some(row) = self.rows.last_mut() {
                row.character_colors = Some(colors);
            }
        }
    }
    fn lines(&mut self, rect: Rect, text: &str, action: Option<Action>, muted: bool) {
        for (index, line) in wrap(text, rect.width)
            .into_iter()
            .take(usize::from(rect.height))
            .enumerate()
        {
            self.row(
                Rect::new(rect.x, rect.y + index as u16, rect.width, 1),
                line,
                None,
                false,
                muted,
            );
        }
        if let Some(action) = action {
            self.hits.push(Hit { rect, action });
        }
    }
    fn editor(&mut self, rect: Rect, editor: &Editor, action: Option<Action>, focused: bool) {
        self.editor_scrolled(rect, editor, action, focused, None, 0);
    }
    /// Draws an editor's wrapped rows. `scroll` pins the viewport; otherwise it
    /// moves from the `previous` frame's offset only as far as the caret needs.
    fn editor_scrolled(
        &mut self,
        rect: Rect,
        editor: &Editor,
        action: Option<Action>,
        focused: bool,
        scroll: Option<usize>,
        previous: usize,
    ) -> (usize, usize) {
        if rect.width == 0 || rect.height == 0 {
            return (0, 0);
        }
        let lines = wrap_ranges(&editor.text, rect.width);
        let (cursor_row, cursor_column) = wrapped_position(&editor.text, &lines, editor.cursor);
        let count = lines.len();
        let height = usize::from(rect.height);
        let skip = scroll
            .unwrap_or(if cursor_row < previous {
                cursor_row
            } else {
                previous.max((cursor_row + 1).saturating_sub(height))
            })
            .min(count.saturating_sub(height));
        let selection = editor.selection();
        for (index, line) in lines.iter().enumerate().skip(skip).take(height) {
            let row = Rect::new(rect.x, rect.y + (index - skip) as u16, rect.width, 1);
            let text = &editor.text[line.clone()];
            self.row(row, display(text), None, focused, false);
            if let Some(selection) = &selection {
                let newline = editor.text[line.end..].starts_with('\n');
                self.select(row, text, line.start, selection, newline);
            }
        }
        if let Some(action) = action {
            self.hits.push(Hit { rect, action });
        }
        if focused && (skip..skip + height).contains(&cursor_row) {
            self.cursor = Some(crate::protocol::CursorState {
                x: rect.x + cursor_column.min(usize::from(rect.width - 1)) as u16,
                y: rect.y + (cursor_row - skip) as u16,
                visible: true,
                shape: 2,
            });
        }
        (skip, count)
    }
    /// Highlights the cells of one displayed row inside `selection`, a byte
    /// range in the source whose row text begins at byte `start`. A selection
    /// running through a hard line end also marks the cell after the text.
    fn select(
        &mut self,
        row: Rect,
        text: &str,
        start: usize,
        selection: &Range<usize>,
        newline: bool,
    ) {
        let end = start + text.len();
        let through = newline && selection.start <= end && selection.end > end;
        if selection.start > end || selection.end < start {
            return;
        }
        let width = usize::from(row.width);
        // History offsets can predate a re-wrap; count whole characters only.
        let cells_before = |offset: usize| {
            (0..=offset - start)
                .rev()
                .find_map(|index| text.get(..index))
                .map_or(0, cells)
        };
        let left = cells_before(selection.start.max(start)).min(width);
        let right = (cells_before(selection.end.min(end)) + usize::from(through)).min(width);
        if right > left {
            self.selection.push(Rect::new(
                row.x + left as u16,
                row.y,
                (right - left) as u16,
                1,
            ));
        }
    }
}

/// Display-only filtering: never place escape/control bytes in a terminal cell.
pub(super) fn display(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c == '\t' {
                ' '
            } else if c.is_control() {
                '�'
            } else {
                c
            }
        })
        .collect()
}
/// Terminal cells for one source character after `display` filtering.
pub(super) fn cell_width(c: char) -> usize {
    if c == '\t' || c.is_control() {
        1
    } else {
        c.width().unwrap_or(0)
    }
}
fn cells(text: &str) -> usize {
    text.chars().map(cell_width).sum()
}
/// Word-wraps `text` into byte ranges, one per terminal row. Every byte except
/// the newline separators belongs to exactly one row, so cursors and
/// selections map back to the source. Spaces hang past the right edge instead
/// of starting a row, words longer than a row break at the edge, and wide
/// (CJK) characters may break on either side.
pub(super) fn wrap_ranges(text: &str, width: u16) -> Vec<Range<usize>> {
    if width == 0 {
        return Vec::new();
    }
    let width = usize::from(width);
    let mut lines = Vec::new();
    let mut base = 0;
    for source in text.split('\n') {
        let mut start = base;
        let mut used = 0;
        let mut breakpoint = None;
        for (offset, c) in source.char_indices() {
            let index = base + offset;
            let size = cell_width(c);
            if c == ' ' || c == '\t' {
                used += size;
                breakpoint = Some(index + c.len_utf8());
                continue;
            }
            if size > 1 {
                breakpoint = Some(index);
            }
            if used + size > width && index > start {
                match breakpoint.filter(|point| *point > start) {
                    Some(point) => {
                        lines.push(start..point);
                        used = cells(&text[point..index]);
                        start = point;
                    }
                    None => {
                        lines.push(start..index);
                        used = 0;
                        start = index;
                    }
                }
                breakpoint = None;
                if used + size > width && index > start {
                    lines.push(start..index);
                    used = 0;
                    start = index;
                }
            }
            used += size;
            if size > 1 {
                breakpoint = Some(index + c.len_utf8());
            }
        }
        base += source.len();
        lines.push(start..base);
        base += 1;
    }
    lines
}
pub(super) fn wrap(text: &str, width: u16) -> Vec<String> {
    wrap_ranges(text, width)
        .into_iter()
        .map(|range| display(&text[range]))
        .collect()
}
/// Row and cell column of a byte offset within wrapped rows. An offset on a
/// soft-wrap boundary belongs to the following row, where typing continues.
pub(super) fn wrapped_position(
    text: &str,
    lines: &[Range<usize>],
    offset: usize,
) -> (usize, usize) {
    let row = lines
        .iter()
        .rposition(|line| line.start <= offset)
        .unwrap_or(0);
    let column = lines.get(row).map_or(0, |line| {
        cells(&text[line.start..offset.clamp(line.start, line.end)])
    });
    (row, column)
}
/// Byte offset of the cell at `column` in `text`: before the character there,
/// or after it when `inclusive` (a forward drag includes the cell under the
/// pointer). Columns past the end map to the end of the text.
pub(super) fn cell_offset(text: &str, column: usize, inclusive: bool) -> usize {
    let mut used = 0;
    for (index, c) in text.char_indices() {
        used += cell_width(c);
        if used > column {
            return if inclusive {
                index + c.len_utf8()
            } else {
                index
            };
        }
    }
    text.len()
}
pub(super) fn provider(provider: Provider) -> &'static str {
    match provider {
        Provider::Codex => "Codex",
        Provider::ClaudeCode => "Claude Code",
        Provider::Cursor => "Cursor",
    }
}
pub(super) fn status(status: RuntimeStatus) -> &'static str {
    match status {
        RuntimeStatus::Idle => "Idle",
        RuntimeStatus::Working => "Working",
        RuntimeStatus::Blocked => "Blocked",
        RuntimeStatus::Launching => "Not ready",
        RuntimeStatus::Unavailable => "Unavailable",
    }
}

fn agent_status(agent: &Agent) -> &'static str {
    if !agent.hook_setup_confirmed && !agent.session_binding_invalidated && !agent.deletion_pending
    {
        "Not ready"
    } else {
        status(agent.status)
    }
}

fn animated_agent_status_colors(agent: &Agent, phase: u8) -> Option<Vec<Color>> {
    let word = agent_status(agent);
    match word {
        "Working" => {
            let head = usize::from(phase) % word.len();
            Some(
                word.chars()
                    .enumerate()
                    .map(|(index, _)| {
                        if index == head {
                            ACCENT
                        } else if index.abs_diff(head) == 1 {
                            WORKING_MID
                        } else {
                            WORKING_DIM
                        }
                    })
                    .collect(),
            )
        }
        "Blocked" => {
            let color = match phase % 6 {
                0 | 5 => BLOCKED_DIM,
                2 | 3 => BLOCKED_BRIGHT,
                _ => BLOCKED_MID,
            };
            Some(vec![color; word.len()])
        }
        _ => None,
    }
}
impl BusUi {
    pub fn cursor(&self) -> Option<crate::protocol::CursorState> {
        self.view.cursor.clone()
    }
    pub fn compute_view(&mut self, cols: u16, rows: u16) {
        let layout = layout(cols, rows);
        let sidebar = layout.sidebar;
        let main = layout.pane_surface;
        let mut view = View {
            sidebar,
            ..View::default()
        };
        let sw = sidebar.width.saturating_sub(3);
        let rooms: Vec<_> = self.snapshot.state.rooms().collect();
        // Measure logical sidebar rows once; only visible rows become labels
        // and hit targets. Expanded paths supply both height and display lines.
        let agents: Vec<_> = self
            .snapshot
            .state
            .agents()
            .filter(|a| Some(a.room_id) == self.room)
            .map(|agent| {
                let paths = if agent.details_disclosed {
                    wrap(&agent.cwd.to_string_lossy(), sw)
                } else {
                    Vec::new()
                };
                let height = 3
                    + usize::from(agent.details_disclosed && agent.branch.is_some())
                    + paths.len();
                (agent, paths, height)
            })
            .collect();
        let content_height =
            6 + rooms.len() + agents.iter().map(|(_, _, height)| height).sum::<usize>();
        let footer = if self.force_exit_available {
            2
        } else {
            u16::from(self.quitting.is_some())
        };
        view.sidebar_body = Rect::new(0, 0, sidebar.width, rows.saturating_sub(footer));
        view.sidebar_max_scroll =
            content_height.saturating_sub(usize::from(view.sidebar_body.height));
        self.sidebar_scroll = self.sidebar_scroll.min(view.sidebar_max_scroll);
        let offset = self.sidebar_scroll;
        let visible_end = offset + usize::from(view.sidebar_body.height);
        let at = |x, y: usize, width| {
            if (offset..visible_end).contains(&y) {
                Rect::new(x, (y - offset) as u16, width, 1)
            } else {
                Rect::default()
            }
        };
        view.row(at(1, 1, sw), "BUSSES", None, false, true);
        view.row(
            at(sidebar.width.saturating_sub(3), 1, 1),
            "+",
            Some(Action::NewRoom),
            false,
            false,
        );
        let mut y = 3usize;
        for room in &rooms {
            let rect = at(1, y, sw.saturating_sub(2));
            y += 1;
            if rect.height == 0 {
                continue;
            }
            let label = if room.unread_count > 0 {
                format!("# {}  {}", room.name, room.unread_count)
            } else {
                format!("# {}", room.name)
            };
            if let Some(rename) = self
                .rename
                .as_ref()
                .filter(|r| r.target == RenameTarget::Room(room.id))
            {
                view.editor(rect, &rename.editor, Some(Action::Room(room.id)), true);
            } else {
                view.row(
                    rect,
                    label,
                    Some(Action::Room(room.id)),
                    Some(room.id) == self.room && self.terminal.is_none(),
                    false,
                );
            }
            view.row(
                at(sidebar.width.saturating_sub(3), y - 1, 1),
                "×",
                Some(Action::Delete(DeleteTarget::Room(room.id))),
                false,
                true,
            );
        }
        view.sidebar_divider = at(1, y, sw);
        y += 1;
        view.row(at(1, y, sw), "AGENTS", None, false, true);
        view.row(
            at(sidebar.width.saturating_sub(3), y, 1),
            "+",
            Some(Action::NewAgent),
            false,
            false,
        );
        y += 2;
        for (agent, paths, height) in agents {
            if y >= visible_end {
                break;
            }
            if y + height <= offset {
                y += height;
                continue;
            }
            let right = agent_status(agent);
            let name_width = sw.saturating_sub(right.len() as u16 + 3);
            let rect = at(1, y, name_width);
            if let Some(rename) = self
                .rename
                .as_ref()
                .filter(|r| r.target == RenameTarget::Agent(agent.id))
            {
                view.editor(rect, &rename.editor, Some(Action::Agent(agent.id)), true);
            } else {
                view.row(
                    rect,
                    &agent.name,
                    Some(Action::Agent(agent.id)),
                    self.terminal == Some(agent.id),
                    false,
                );
                let [r, g, b] = agent.color;
                view.color_last_row(rect, Color::Rgb(r, g, b));
            }
            let status_rect = at(
                sidebar.width.saturating_sub(right.len() as u16 + 4),
                y,
                right.len() as u16,
            );
            view.row(
                status_rect,
                right,
                Some(Action::Agent(agent.id)),
                false,
                true,
            );
            if let Some(colors) = animated_agent_status_colors(agent, self.status_animation_phase) {
                view.color_last_row_characters(status_rect, colors);
            }
            view.row(
                at(sidebar.width.saturating_sub(3), y, 1),
                "×",
                Some(Action::Delete(DeleteTarget::Agent(agent.id))),
                false,
                true,
            );
            y += 1;
            view.row(
                at(1, y, sw.saturating_sub(2)),
                provider(agent.provider),
                Some(Action::Agent(agent.id)),
                false,
                true,
            );
            view.row(
                at(sidebar.width.saturating_sub(3), y, 1),
                if agent.details_disclosed { "v" } else { ">" },
                Some(Action::Details(agent.id)),
                false,
                true,
            );
            y += 1;
            if agent.details_disclosed {
                if let Some(branch) = &agent.branch {
                    view.row(at(1, y, sw), branch, None, false, true);
                    y += 1;
                }
                for line in paths {
                    view.row(at(1, y, sw), line, None, false, true);
                    y += 1;
                }
            }
            y += 1;
        }
        view.row(
            Rect::new(1, rows.saturating_sub(2), sw, 1),
            if self.force_exit_available {
                "Unsaved: Ctrl+Shift+Q exits"
            } else {
                ""
            },
            None,
            false,
            true,
        );
        view.row(
            Rect::new(1, rows.saturating_sub(1), sw, 1),
            if self.quitting.is_some() {
                "Saving before exit…"
            } else if self.force_exit_available {
                "Ctrl+C retries saving"
            } else {
                ""
            },
            None,
            false,
            true,
        );
        if let Some(form) = &self.form {
            self.form_view(&mut view, main, form);
        } else if self.terminal.is_none() {
            self.room_view(&mut view, main);
        } else {
            view.lines(
                Rect::new(main.x + 2, 2, main.width.saturating_sub(4), 3),
                "Opening agent… Room remains available in the sidebar.",
                None,
                true,
            );
            if let Some(error) = self.visible_error() {
                view.lines(
                    Rect::new(main.x + 2, 6, main.width.saturating_sub(4), 8),
                    error,
                    None,
                    false,
                );
            }
        }
        if self.deletion.is_some() {
            self.delete_view(&mut view, Rect::new(0, 0, cols, rows));
        }
        self.view = view;
    }
    fn room_view(&mut self, view: &mut View, main: Rect) {
        let Some(room) = self.room.and_then(|id| self.snapshot.state.room(id)) else {
            view.lines(
                Rect::new(main.x + 2, 2, main.width.saturating_sub(4), 3),
                "No rooms. Click BUSSES + or press Ctrl+Shift+R to add one.",
                None,
                true,
            );
            return;
        };
        let Some(local) = self.locals.get(&room.id) else {
            return;
        };
        let queued: usize = self
            .snapshot
            .state
            .agents()
            .filter(|a| a.room_id == room.id)
            .map(|a| self.snapshot.state.queued_requests(a.id).len())
            .sum();
        let errors = self
            .snapshot
            .state
            .agents()
            .filter(|a| a.room_id == room.id)
            .filter_map(|a| {
                a.actionable_error
                    .as_ref()
                    .map(|e| format!("{}: {e}", a.name))
            })
            .collect::<Vec<_>>()
            .join(" · ");
        let notice = self
            .visible_error()
            .or_else(|| (!errors.is_empty()).then_some(errors.as_str()));
        let search_status = self
            .history_search
            .as_ref()
            .map(|search| (search.query.clone(), search.selected));
        let search_status = search_status.map(|(query, selected)| {
            let total = self
                .room
                .map(|room| self.filtered_history(room, &query).len())
                .unwrap_or(0)
                .max(1);
            (query, selected, total)
        });
        let status = notice
            .map(str::to_owned)
            .or_else(|| {
                search_status.as_ref().map(|(query, selected, total)| {
                    format!("search {query}  {}/{total}", selected + 1)
                })
            })
            .unwrap_or_else(|| {
                if self.send_intent.is_some() {
                    "Saving latest draft before sending…".into()
                } else if queued > 0 {
                    format!("{queued} queued requests")
                } else {
                    String::new()
                }
            });
        let x = main.x + 2;
        let width = main.width.saturating_sub(4);
        let bottom = main.bottom();
        let composer_bottom = bottom.saturating_sub(u16::from(!status.is_empty()));
        let recipients = super::recipients::layout(
            local.recipients.iter().filter_map(|id| {
                self.snapshot
                    .state
                    .agent(*id)
                    .map(|agent| (agent.id, agent.name.as_str()))
            }),
            width,
        );
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
        self.recipient_scroll = self.recipient_scroll.min(view.recipient_max_scroll);
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
        if composer_y > 2 {
            view.row(
                Rect::new(x, 1, width, 1),
                format!("# {}", room.name),
                None,
                false,
                false,
            );
        }
        let note_height = 3.min(composer_y.saturating_sub(5));
        if note_height > 0 {
            view.notes_box =
                Rect::new(main.x + 1, 2, main.width.saturating_sub(2), note_height + 2);
        }
        // The expanded draft may hide the top section. Never paint a history
        // separator over its editor or reserve rows from full-screen editing.
        let history_y = 8;
        if history_y - 1 < view.composer_box.y {
            view.history_divider =
                Rect::new(main.x + 1, history_y - 1, main.width.saturating_sub(2), 1);
        }
        view.notes = Rect::new(x, 3, width, note_height);
        (view.notes_scroll, _) = view.editor_scrolled(
            view.notes,
            &local.notes,
            Some(Action::Notes),
            self.notes_focus,
            None,
            self.view.notes_scroll,
        );
        if note_height > 0 && local.notes.text.is_empty() {
            view.row(
                Rect::new(x, 3, width, 1),
                "Add notes… (F3)",
                Some(Action::Notes),
                false,
                true,
            );
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis() as u64);
        let content = self.history.lines(
            &self.snapshot.state,
            room,
            width,
            self.snapshot.revision,
            now,
        );
        view.history = Rect::new(
            main.x,
            history_y,
            main.width,
            view.composer_box.y.saturating_sub(history_y),
        );
        view.history_max_scroll = content
            .len()
            .saturating_sub(usize::from(view.history.height));
        self.main_scroll = if self.history_follow_tail {
            view.history_max_scroll
        } else {
            self.main_scroll.min(view.history_max_scroll)
        };
        view.history_text = Rect::new(x, history_y, width, view.history.height);
        let selection = self
            .history_selection
            .map(|(anchor, head)| (anchor.min(head), anchor.max(head)));
        for (index, line) in content
            .iter()
            .skip(self.main_scroll)
            .take(usize::from(view.composer_box.y.saturating_sub(history_y)))
            .enumerate()
        {
            let rect = Rect::new(x, history_y + index as u16, width, 1);
            view.row(
                rect,
                &line.text,
                line.action.clone(),
                false,
                matches!(line.tone, super::history::Tone::Muted),
            );
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
            let mut column = 0u16;
            for (text, tone) in &line.spans {
                let span_width = unicode_width::UnicodeWidthStr::width(text.as_str()) as u16;
                let color = match tone {
                    super::history::Tone::You => Some(ACCENT),
                    super::history::Tone::Agent(id) => {
                        self.snapshot.state.agent(*id).map(|agent| {
                            let [r, g, b] = agent.color;
                            Color::Rgb(r, g, b)
                        })
                    }
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
        }
        view.lines(
            Rect::new(x, bottom.saturating_sub(1), width, 1),
            &status,
            None,
            true,
        );
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
                    let [r, g, b] = agent.color;
                    Color::Rgb(r, g, b)
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
        let editor_y = composer_y + bar_height + 1;
        view.composer = Rect::new(x, editor_y, width, text_height);
        view.composer_max_height = max_height;
        (view.composer_scroll, view.composer_rows) = view.editor_scrolled(
            view.composer,
            &local.text,
            Some(Action::Composer),
            !self.notes_focus && !self.recipient_menu && self.rename.is_none(),
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
        let mut files: Vec<_> = room
            .draft
            .files
            .iter()
            .cloned()
            .map(|path| (path, false))
            .collect();
        for pending in &self.failed {
            if let crate::bus::runtime::BusCommand::AttachFile(id, path) = &pending.command {
                if *id != room.id {
                    continue;
                }
                files.push((std::path::PathBuf::from(path), true));
            }
        }
        self.file_scroll = self.file_scroll.min(files.len().saturating_sub(1));
        if !files.is_empty() {
            view.files = Rect::new(x, editor_y + text_height, width, 1);
        }
        // Reserve overflow controls before placing chips, so even one very long
        // filename cannot consume the route to later successful/failed files.
        let controls = files.len() > 1;
        let mut chip_x = x + if controls { 3 } else { 0 };
        let chip_right = x + width.saturating_sub(if controls { 3 } else { 0 });
        let mut shown = 0;
        for (path, failed) in files.iter().skip(self.file_scroll) {
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
            let size = unicode_width::UnicodeWidthStr::width(label.as_str()) as u16;
            view.row(
                Rect::new(chip_x, editor_y + text_height, size, 1),
                label,
                Some(Action::RemoveFile(path.clone())),
                false,
                true,
            );
            chip_x += size;
            shown += 1;
        }
        if controls && self.file_scroll > 0 {
            view.row(
                Rect::new(x, editor_y + text_height, 2, 1),
                "<",
                Some(Action::ScrollFiles(false)),
                false,
                true,
            );
        }
        if controls && self.file_scroll + shown < files.len() {
            view.row(
                Rect::new(x + width.saturating_sub(2), editor_y + text_height, 2, 1),
                ">",
                Some(Action::ScrollFiles(true)),
                false,
                true,
            );
        }
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
        }
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
                            status_name(a.status)
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
        }
    }
    fn delete_view(&self, view: &mut View, area: Rect) {
        let Some(dialog) = &self.deletion else {
            return;
        };
        let (title, message) = match dialog.target {
            DeleteTarget::Room(id) => {
                let name = self
                    .snapshot
                    .state
                    .room(id)
                    .map_or("room", |room| room.name.as_str());
                let count = self
                    .snapshot
                    .state
                    .agents()
                    .filter(|agent| agent.room_id == id)
                    .count();
                (format!("Delete room \"{name}\"?"), format!("This will close its {count} agents and permanently delete this room’s session data."))
            }
            DeleteTarget::Agent(id) => {
                let name = self
                    .snapshot
                    .state
                    .agent(id)
                    .map_or("agent", |agent| agent.name.as_str());
                (format!("Delete agent \"{name}\"?"), "This will close the agent and permanently delete its session data from this room.".into())
            }
        };
        let width = area.width.saturating_sub(2).min(66);
        let text_width = width.saturating_sub(4);
        let mut body = wrap(dialog.error.as_ref().map_or(message.as_str(), |error| {
            if error.contains("server") {
                "Bus server needs an update before deletion can finish. Session data was kept."
            } else {
                "Deletion did not finish. Session data was kept. Check the terminal before trying again."
            }
        }), text_width);
        if dialog.command_id.is_some() {
            body.push("Stopping sessions…".into());
        }
        let height = (body.len().saturating_add(6)).min(usize::from(area.height)) as u16;
        let rect = Rect::new(
            area.x + area.width.saturating_sub(width) / 2,
            area.y + area.height.saturating_sub(height) / 2,
            width,
            height,
        );
        view.dialog = rect;
        view.dialog_rows_start = view.rows.len();
        view.hits.clear();
        view.cursor = None;
        let x = rect.x + 2;
        view.row(
            Rect::new(x, rect.y + 1, text_width, 1),
            title,
            None,
            false,
            false,
        );
        for (index, line) in body
            .into_iter()
            .take(usize::from(height.saturating_sub(5)))
            .enumerate()
        {
            view.row(
                Rect::new(x, rect.y + 2 + index as u16, text_width, 1),
                line,
                None,
                false,
                true,
            );
        }
        let y = rect.bottom().saturating_sub(2);
        let ready = dialog.command_id.is_none();
        if dialog.error.is_none() {
            view.row(
                Rect::new(x, y, text_width.min(12), 1),
                "Cancel (Esc)",
                ready.then_some(Action::CancelDelete),
                false,
                !ready,
            );
        }
        view.row(
            Rect::new(x + 16, y, text_width.saturating_sub(16).min(10), 1),
            "OK (Enter)",
            ready.then_some(Action::ConfirmDelete),
            false,
            !ready,
        );
    }
    fn form_view(&self, view: &mut View, main: Rect, form: &Form) {
        let x = main.x + 3;
        let width = main.width.saturating_sub(6);
        let mut y = 3;
        match form {
            Form::Help { scroll } => {
                view.row(
                    Rect::new(x, 1, width, 1),
                    "Keyboard shortcuts",
                    None,
                    false,
                    false,
                );
                view.help = Rect::new(x, 3, width, main.height.saturating_sub(6));
                let lines = wrap(super::help::TEXT, width);
                view.help_max_scroll = lines.len().saturating_sub(usize::from(view.help.height));
                view.help_scroll = (*scroll).min(view.help_max_scroll);
                for (index, line) in lines
                    .into_iter()
                    .skip(view.help_scroll)
                    .take(usize::from(view.help.height))
                    .enumerate()
                {
                    view.row(
                        Rect::new(x, 3 + index as u16, width, 1),
                        line,
                        None,
                        false,
                        false,
                    );
                }
                view.row(
                    Rect::new(x, main.bottom().saturating_sub(2), width, 1),
                    "Close (Esc / Enter) · ↑↓ / PgUp/Dn scroll",
                    Some(Action::Cancel),
                    false,
                    true,
                );
                return;
            }
            Form::Room(editor) => {
                view.row(Rect::new(x, y, width, 1), "Room name", None, false, true);
                view.editor(Rect::new(x, y + 1, width, 1), editor, None, true);
                y += 4;
            }
            Form::Files(editor) => {
                view.row(
                    Rect::new(x, y, width, 1),
                    "Local file path",
                    None,
                    false,
                    true,
                );
                view.editor(Rect::new(x, y + 1, width, 1), editor, None, true);
                y += 3;
                view.lines(
                    Rect::new(x, y, width, 2),
                    "Choose a real local file. Paths are references; contents are not uploaded.",
                    None,
                    true,
                );
                y += 3;
            }
            Form::Agent {
                name,
                provider: selected,
                provider_cursor: _,
                cwd,
                args,
                field,
            } => {
                for (index, label, editor) in [
                    (0, "Name", Some(name)),
                    (1, "Agent", None),
                    (2, "PWD", Some(cwd)),
                    (3, "Additional launch args (optional)", Some(args)),
                ] {
                    view.row(
                        Rect::new(x, y, width, 1),
                        label,
                        Some(Action::Field(index)),
                        false,
                        true,
                    );
                    y += 1;
                    if let Some(editor) = editor {
                        view.editor(
                            Rect::new(x, y, width, 1),
                            editor,
                            Some(Action::Field(index)),
                            *field == index,
                        );
                    } else {
                        view.row(
                            Rect::new(x, y, width, 1),
                            format!("< {} >", selected.map_or("Choose model", provider)),
                            Some(Action::Field(1)),
                            *field == 1,
                            false,
                        );
                    }
                    y += 2;
                }
            }
            Form::Consent { notice, .. } => {
                let text = format!(
                    "{}\n{}\n\nAdd confirms writing these Bus-owned hook entries. Bus will become ready automatically; this does not approve provider permissions.",
                    notice.message,
                    notice.path.display()
                );
                let height = wrap(&text, width).len() as u16;
                view.lines(Rect::new(x, y, width, height), &text, None, false);
                y += height + 2;
            }
        }
        view.row(
            Rect::new(x, y, 14.min(width), 1),
            "Cancel (Esc)",
            Some(Action::Cancel),
            false,
            true,
        );
        view.row(
            Rect::new(x + 16, y, 24.min(width.saturating_sub(16)), 1),
            "Add (Enter)",
            Some(Action::Add),
            false,
            false,
        );
        y += 2;
        if let Some(error) = self.visible_error() {
            view.lines(Rect::new(x, y, width, 3), error, None, false);
            y += 4;
        }
        let remaining = main.bottom().saturating_sub(y + 2) as usize;
        let start = self
            .suggestions
            .selected
            .saturating_sub(remaining.saturating_sub(1));
        for (index, suggestion) in self
            .suggestions
            .entries
            .iter()
            .enumerate()
            .skip(start)
            .take(remaining)
        {
            view.row(
                Rect::new(x, y + (index - start) as u16, width, 1),
                format!(
                    "{}{}",
                    suggestion.path.display(),
                    if suggestion.is_directory { "/" } else { "" }
                ),
                Some(Action::Suggestion(index)),
                index == self.suggestions.selected,
                true,
            );
        }
        if let Form::Agent {
            provider_cursor,
            field: 1,
            ..
        } = form
        {
            for (index, kind) in [Provider::Codex, Provider::ClaudeCode, Provider::Cursor]
                .into_iter()
                .enumerate()
            {
                view.row(
                    Rect::new(x, 8 + index as u16, width, 1),
                    provider(kind),
                    Some(Action::Provider(kind)),
                    *provider_cursor == kind,
                    false,
                );
            }
        }
    }
    pub fn render(&self, buffer: &mut Buffer) {
        buffer.set_style(
            buffer.area,
            Style::default()
                .fg(Color::Rgb(222, 222, 226))
                .bg(Color::Rgb(24, 24, 28)),
        );
        // Fixed-count borders/dividers per client frame, never per agent/pane.
        for rect in [self.view.composer_box, self.view.notes_box] {
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(ACCENT))
                .render(rect.intersection(buffer.area), buffer);
        }
        for (rect, color) in &self.view.recipient_chips {
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(*color))
                .render(rect.intersection(buffer.area), buffer);
        }
        for rect in [
            self.view.composer_divider,
            self.view.history_divider,
            self.view.sidebar_divider,
        ] {
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(ACCENT))
                .render(rect.intersection(buffer.area), buffer);
        }
        let divider = self.view.composer_divider;
        if divider.width >= 2 && divider.height > 0 {
            for (x, symbol) in [(divider.x, "├"), (divider.right() - 1, "┤")] {
                if buffer.area.contains((x, divider.y).into()) {
                    buffer.set_stringn(x, divider.y, symbol, 1, Style::default().fg(ACCENT));
                }
            }
        }
        // Selection tints the rows it covers but stays beneath a dialog.
        fn paint_selection(selection: &[Rect], buffer: &mut Buffer) {
            for rect in selection {
                let rect = rect.intersection(buffer.area);
                for y in rect.top()..rect.bottom() {
                    for x in rect.left()..rect.right() {
                        buffer[(x, y)].set_bg(SELECTION);
                    }
                }
            }
        }
        let mut selection_painted = false;
        for (index, row) in self.view.rows.iter().enumerate() {
            if index == self.view.dialog_rows_start && self.view.dialog.width > 0 {
                paint_selection(&self.view.selection, buffer);
                selection_painted = true;
                let rect = self.view.dialog.intersection(buffer.area);
                Clear.render(rect, buffer);
                Block::default()
                    .borders(Borders::ALL)
                    .style(Style::default().bg(Color::Rgb(24, 24, 28)))
                    .border_style(Style::default().fg(ACCENT))
                    .render(rect, buffer);
            }
            if row.y >= buffer.area.bottom() || row.x >= buffer.area.right() {
                continue;
            }
            let style = Style::default()
                .fg(row.color.unwrap_or(if row.muted {
                    Color::Rgb(145, 148, 159)
                } else {
                    Color::Rgb(222, 222, 226)
                }))
                .bg(if row.selected {
                    Color::Rgb(46, 48, 58)
                } else {
                    Color::Rgb(24, 24, 28)
                });
            buffer.set_stringn(
                row.x,
                row.y,
                display(&row.text),
                row.width.min(buffer.area.right() - row.x) as usize,
                style,
            );
            if let Some(colors) = &row.character_colors {
                let mut x = row.x;
                let right = row.x.saturating_add(row.width).min(buffer.area.right());
                for (index, character) in display(&row.text).chars().enumerate() {
                    let width = cell_width(character) as u16;
                    if width == 0 {
                        continue;
                    }
                    if x.saturating_add(width) > right {
                        break;
                    }
                    let color = colors
                        .get(index)
                        .copied()
                        .unwrap_or(style.fg.unwrap_or_default());
                    buffer.set_stringn(
                        x,
                        row.y,
                        character.to_string(),
                        usize::from(width),
                        style.fg(color),
                    );
                    x = x.saturating_add(width);
                }
            }
            if row.color.is_none() && row.text.starts_with('#') {
                buffer.set_stringn(row.x, row.y, "#", 1, Style::default().fg(ACCENT));
            }
        }
        if !selection_painted {
            paint_selection(&self.view.selection, buffer);
        }
        let sidebar = self.view.sidebar.intersection(buffer.area);
        if sidebar.width > 0 {
            for y in sidebar.y..sidebar.bottom() {
                if !self.view.dialog.contains((sidebar.right() - 1, y).into()) {
                    buffer.set_stringn(sidebar.right() - 1, y, "│", 1, Style::default().fg(ACCENT));
                }
            }
        }
    }
}
fn status_name(value: RuntimeStatus) -> &'static str {
    status(value)
}
pub(in crate::client::shell) fn layout(
    cols: u16,
    rows: u16,
) -> crate::client::shell::ClientShellLayout {
    let width = 28.min(cols / 3).max(1).min(cols);
    crate::client::shell::ClientShellLayout {
        sidebar: Rect::new(0, 0, width, rows),
        pane_surface: Rect::new(width, 0, cols - width, rows),
        tab_bar: Rect::default(),
        mobile_header: Rect::default(),
    }
}
