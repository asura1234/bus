//! Forms, the help screen, deletion confirmation and error dialogs.
use super::super::{chat_search::ChatSearch, deletion::DeleteTarget, forms::Form, BusUi};
use super::geometry::agent_form_gap;
use super::text::{provider, wrap};
use super::{Action, View};
use ratatui::layout::Rect;

/// The chat-search panel: a border, the query row, the key hint, a border.
// Border, the 3-row query field, the key hint, border.
pub(super) const SEARCH_BOX_HEIGHT: u16 = 6;
/// The widest a centered dialog grows on a wide screen.
const DIALOG_WIDTH: u16 = 66;
const SEARCH_LABEL: &str = "Find ";
/// Enter steps to older matches (up the history), as a find in a chat starts
/// from the newest message. Down also goes newer: it works in every terminal,
/// while Shift+Enter needs one that reports it apart from Enter.
const SEARCH_HINTS: [&str; 3] = [
    "Enter/↑ older · Shift+Enter/↓ newer · Esc close",
    "Enter/↑ older · ↓ newer · Esc close",
    "↑ older · ↓ newer · Esc",
];

impl BusUi {
    pub(super) fn delete_view(&self, view: &mut View, area: Rect) {
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
                let state = &self.snapshot.state;
                let count = state.agents().filter(|agent| agent.room_id == id).count();
                // The orchestrator lives in MASTER but exists only for this room.
                let orchestrator = state.orchestrator_of(id).map_or(String::new(), |agent| {
                    format!(" and its orchestrator \"{}\" in MASTER", agent.name)
                });
                (format!("Delete room \"{name}\"?"), format!("This will close its {count} agents{orchestrator} and permanently delete this room’s session data."))
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
        let text_width = dialog_text_width(area);
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
        let (x, y) = dialog_frame(view, area, title, body);
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

    pub(super) fn alert_view(&self, view: &mut View, area: Rect) {
        let Some(alert) = &self.alert else {
            return;
        };
        let text_width = dialog_text_width(area);
        let body = wrap(&alert.message, text_width);
        let (x, y) = dialog_frame(view, area, alert.title.clone(), body);
        view.row(
            Rect::new(x, y, text_width.min(10), 1),
            "OK (Enter)",
            Some(Action::DismissAlert),
            false,
            false,
        );
    }

    pub(super) fn form_view(&self, view: &mut View, main: Rect, form: &Form) {
        let x = main.x + 3;
        let width = main.width.saturating_sub(6);
        let mut y = 3;
        match form {
            Form::Help { scroll } => {
                self.help_view(view, main, *scroll);
                return;
            }
            Form::Settings => {
                self.settings_view(view, main);
                return;
            }
            Form::FeishuSetting { cli, editor } => {
                view.row(
                    Rect::new(x, y, width, 1),
                    if *cli {
                        "lark-cli path (empty uses PATH)"
                    } else {
                        "Feishu recipient open_id"
                    },
                    None,
                    false,
                    true,
                );
                view.editor(Rect::new(x, y + 1, width, 1), editor, None, true);
                y += 4;
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
            Form::Agent { .. } => {
                y = self.agent_fields(view, main, form);
            }
            Form::Consent { notice, .. } => {
                let text = format!("{}\n{}", notice.message, notice.path.display());
                let height = wrap(&text, width).len() as u16;
                view.lines(Rect::new(x, y, width, height), &text, None, false);
                y += height + 2;
            }
        }
        self.form_footer(view, main, form, y);
    }
    fn help_view(&self, view: &mut View, main: Rect, scroll: usize) {
        let x = main.x + 3;
        let width = main.width.saturating_sub(6);
        view.row(
            Rect::new(x, 1, width, 1),
            "Keyboard shortcuts",
            None,
            false,
            false,
        );
        view.help = Rect::new(x, 3, width, main.height.saturating_sub(6));
        let lines = wrap(super::super::help::TEXT, width);
        view.help_max_scroll = lines.len().saturating_sub(usize::from(view.help.height));
        view.help_scroll = scroll.min(view.help_max_scroll);
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
    }

    fn agent_fields(&self, view: &mut View, main: Rect, form: &Form) -> u16 {
        let Form::Agent {
            name,
            provider: selected,
            cwd,
            args,
            field,
            ..
        } = form
        else {
            return 3;
        };
        let x = main.x + 3;
        let width = main.width.saturating_sub(6);
        let mut y = 3;
        let gap = agent_form_gap(main, form);
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
            y += gap;
        }

        self.orchestrator_fields(view, main, form, y, gap)
    }

    fn orchestrator_fields(
        &self,
        view: &mut View,
        main: Rect,
        form: &Form,
        mut y: u16,
        gap: u16,
    ) -> u16 {
        let Form::Agent {
            field,
            orchestrates,
            prompt,
            ..
        } = form
        else {
            return y;
        };
        let x = main.x + 3;
        let width = main.width.saturating_sub(6);
        if let Some(choice) = orchestrates {
            view.row(
                Rect::new(x, y, width, 1),
                "Orchestrates room",
                Some(Action::Field(super::super::forms::ORCHESTRATES_FIELD)),
                false,
                true,
            );
            y += 1;
            let target = choice
                .0
                .and_then(|room| self.snapshot.state.room(room))
                .map_or("no work room without an orchestrator", |room| {
                    room.name.as_str()
                });
            view.row(
                Rect::new(x, y, width, 1),
                format!("< {target} >"),
                Some(Action::Orchestrates),
                *field == super::super::forms::ORCHESTRATES_FIELD,
                false,
            );
            y += gap;
        }
        if let Some(prompt) = prompt {
            let index = super::super::forms::PROMPT_FIELD;
            view.row(
                Rect::new(x, y, width, 1),
                "System prompt (Enter adds a line, Ctrl+Enter adds the agent)",
                Some(Action::Field(index)),
                false,
                true,
            );
            y += 1;
            // Leave room for the buttons, validation error and a short PWD list.
            let mut below = if self.visible_error().is_some() { 7 } else { 3 };
            let suggestions = self.suggestions.entries.len().min(3) as u16;
            if suggestions > 0 {
                below += 2 + suggestions;
            }
            let height = main.bottom().saturating_sub(y + below).clamp(1, 16);
            view.editor(
                Rect::new(x, y, width, height),
                &prompt.editor,
                Some(Action::Field(index)),
                *field == index,
            );
            y += height + 1;
        }

        y
    }

    fn form_footer(&self, view: &mut View, main: Rect, form: &Form, mut y: u16) {
        let x = main.x + 3;
        let width = main.width.saturating_sub(6);
        view.row(
            Rect::new(x, y, 14.min(width), 1),
            "Cancel (Esc)",
            Some(Action::Cancel),
            false,
            true,
        );
        view.row(
            Rect::new(x + 16, y, 24.min(width.saturating_sub(16)), 1),
            if matches!(form, Form::FeishuSetting { .. }) {
                "Save (Enter)"
            } else {
                "Add (Enter)"
            },
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
            orchestrates,
            ..
        } = form
        {
            // Below the Agent field's value row.
            let top = 6 + agent_form_gap(main, form);
            for (index, kind) in super::super::forms::provider_choices(orchestrates.is_some())
                .iter()
                .copied()
                .enumerate()
            {
                view.overlay_row(
                    Rect::new(x, top + index as u16, width, 1),
                    provider(kind).to_owned(),
                    Some(Action::Provider(kind)),
                    *provider_cursor == kind,
                );
            }
        }
    }
}

pub(super) fn search_panel(view: &mut View, search: &ChatSearch, x: u16, width: u16, count: usize) {
    let panel = view.search_box;
    if panel.height > 0 {
        let counter = match (search.query.is_empty() && !search.you_only, count) {
            (true, _) => String::new(),
            (false, 0) => "no matches".to_owned(),
            (false, _) => format!("{}/{count}", search.current + 1),
        };
        let counter_width = unicode_width::UnicodeWidthStr::width(counter.as_str()) as u16;
        // The label sits outside the field, dim like the hint, so it
        // never reads as part of the query.
        let label_width = SEARCH_LABEL.len() as u16;
        let field_y = panel.y + 1;
        view.row(
            Rect::new(x, field_y + 1, label_width, 1),
            SEARCH_LABEL,
            None,
            false,
            true,
        );
        // Room for the widest counter, so the field keeps its width
        // as the counter changes.
        let field_width = width.saturating_sub(label_width + "no matches".len() as u16 + 1);
        view.search_field = Rect::new(x + label_width, field_y, field_width, 3);
        let inner = field_width.saturating_sub(2);
        view.row(
            Rect::new(x + label_width + 1, field_y + 1, inner, 1),
            &search.query,
            None,
            false,
            false,
        );
        view.row(
            Rect::new(
                x + width.saturating_sub(counter_width),
                field_y + 1,
                counter_width,
                1,
            ),
            &counter,
            None,
            false,
            true,
        );
        // Keep the filter and its state visible even in a narrow pane.
        let toggle = format!(
            "Tab → You only: {}",
            if search.you_only { "on" } else { "off" }
        );
        let available = usize::from(width)
            .saturating_sub(unicode_width::UnicodeWidthStr::width(toggle.as_str()) + 3);
        let hint = SEARCH_HINTS
            .iter()
            .find(|hint| unicode_width::UnicodeWidthStr::width(**hint) <= available);
        let hint = hint.map_or(toggle.clone(), |hint| format!("{toggle} · {hint}"));
        view.row(Rect::new(x, field_y + 3, width, 1), hint, None, false, true);
        let typed = unicode_width::UnicodeWidthStr::width(search.query.as_str()) as u16;
        view.cursor = Some(crate::protocol::wire::CursorState {
            x: x + label_width + 1 + typed.min(inner.saturating_sub(1)),
            y: field_y + 1,
            visible: true,
            shape: 2,
        });
    }
}

/// The text width of a centered dialog over `area`.
fn dialog_text_width(area: Rect) -> u16 {
    area.width
        .saturating_sub(2)
        .min(DIALOG_WIDTH)
        .saturating_sub(4)
}

/// Draws a centered dialog of `title` over `body` (rows already wrapped to
/// `dialog_text_width`) and returns where its button row starts.
fn dialog_frame(view: &mut View, area: Rect, title: String, body: Vec<String>) -> (u16, u16) {
    let width = area.width.saturating_sub(2).min(DIALOG_WIDTH);
    let text_width = width.saturating_sub(4);
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
    (x, y)
}
