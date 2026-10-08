//! Form editing, orchestrator choices, path completion and submission.
use super::super::{
    editor::Editor,
    forms::{self, Form, Rename, RenameTarget, ORCHESTRATES_FIELD, PROMPT_FIELD},
    render::Action,
    BusUi, Effect,
};
use crate::bus::{
    launch::AddAgent,
    model::{ModelError, RoomId, RoomKind},
    orchestrator::OrchestratorSpec,
    runtime::BusCommand,
};
use crossterm::event::{KeyCode, KeyModifiers};

impl BusUi {
    /// The work rooms a new orchestrator can take: each work room has at most one.
    pub(super) fn orchestratable_rooms(&self) -> Vec<RoomId> {
        let state = &self.snapshot.state;
        state
            .rooms()
            .filter(|room| {
                room.kind == RoomKind::Work
                    && !room.deletion_pending
                    && state.orchestrator_of(room.id).is_none()
            })
            .map(|room| room.id)
            .collect()
    }

    /// Steps the new orchestrator's room through the work rooms without one.
    /// There is no "no room" choice: an orchestrator exists only for its room.
    pub(super) fn cycle_orchestrates(&mut self, forward: bool) {
        let choices = self.orchestratable_rooms();
        if let Some(Form::Agent {
            orchestrates: Some(choice),
            ..
        }) = &mut self.form
        {
            choice.0 = match choice.0.and_then(|c| choices.iter().position(|r| *r == c)) {
                _ if choices.is_empty() => None,
                None => Some(choices[0]),
                Some(index) => Some(
                    choices[if forward {
                        (index + 1) % choices.len()
                    } else {
                        (index + choices.len() - 1) % choices.len()
                    }],
                ),
            };
        }
        self.refill_orchestrator_prompt();
    }

    pub(super) fn start_rename(&mut self, target: RenameTarget) {
        if let RenameTarget::Room(id) = target {
            if self.is_master_room(id) {
                self.error = Some(ModelError::MasterRoomFixed.to_string());
                return;
            }
        }
        let name = match target {
            RenameTarget::Room(id) => self.snapshot.state.room(id).map(|r| r.name.clone()),
            RenameTarget::Agent(id) => self.snapshot.state.agent(id).map(|a| a.name.clone()),
        };
        if let Some(name) = name {
            self.rename = Some(Rename {
                target,
                editor: Editor::new(name),
            });
        }
    }

    pub(super) fn open_form(&mut self, form: Form) {
        self.clear_selection();
        if self.failed.is_empty() {
            // Dismiss only a handled UI operation's matching snapshot copy,
            // not unrelated background/runtime failures or unsaved drafts.
            if self.error.is_some() && self.error == self.snapshot.error {
                self.dismissed_snapshot_error = self.error.clone();
            }
            self.error = None;
        }
        self.form = Some(form);
        self.terminal = None;
        self.recipient_menu = false;
        self.queue(BusCommand::LeaveRoom, Effect::None);
        self.query_paths();
    }

    pub(super) fn form_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        if code == KeyCode::Esc {
            self.action(Action::Cancel);
            return;
        }
        if matches!(self.form, Some(Form::Settings)) {
            self.settings_form_key(code);
            return;
        }
        if matches!(self.form, Some(Form::Help { .. })) {
            self.help_form_key(code);
            return;
        }
        if matches!(code, KeyCode::Up | KeyCode::Down) && !self.suggestions.entries.is_empty() {
            self.suggestions.selected = if code == KeyCode::Up {
                self.suggestions.selected.saturating_sub(1)
            } else {
                (self.suggestions.selected + 1).min(self.suggestions.entries.len() - 1)
            };
            return;
        }
        if self.agent_choice_key(code) {
            return;
        }
        // The system prompt is multi-line: Enter adds a line, Ctrl+Enter adds the agent.
        if code == KeyCode::Enter
            && !modifiers.contains(KeyModifiers::CONTROL)
            && matches!(
                self.form,
                Some(Form::Agent {
                    field: PROMPT_FIELD,
                    ..
                })
            )
        {
            self.insert("\n");
            return;
        }
        if code == KeyCode::Enter && matches!(self.form, Some(Form::Agent { .. })) {
            self.add();
            return;
        }
        if matches!(code, KeyCode::Tab | KeyCode::Enter) && !self.suggestions.entries.is_empty() {
            self.complete_path();
            return;
        }
        if code == KeyCode::Enter && modifiers.contains(KeyModifiers::CONTROL) {
            self.add();
            return;
        }
        if code == KeyCode::Tab || code == KeyCode::BackTab || code == KeyCode::Enter {
            let count = self.form.as_ref().map_or(0, Form::agent_field_count);
            if let Some(Form::Agent { field, .. }) = &mut self.form {
                *field = if code == KeyCode::BackTab {
                    (*field + count - 1) % count
                } else {
                    (*field + 1) % count
                };
                self.query_paths();
                return;
            }
            if code == KeyCode::Enter {
                self.add();
            }
            return;
        }
        if let KeyCode::Char(c) = code {
            if !modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
            {
                self.insert(&c.to_string());
                return;
            }
        }
        // Modified letters are editing chords (Alt+B/F/D, Ctrl+W) or no-ops.
        let edited = self
            .form
            .as_mut()
            .and_then(Form::editor_mut)
            .is_some_and(|editor| {
                let before = editor.cursor;
                editor.key(code, modifiers) || editor.cursor != before
            });
        if edited || !matches!(code, KeyCode::Char(_)) {
            self.refill_orchestrator_prompt();
            self.query_paths();
        }
    }

    fn settings_form_key(&mut self, code: KeyCode) {
        let targets = self.sound_settings_targets();
        match code {
            KeyCode::Up => self.settings_field = self.settings_field.saturating_sub(1),
            KeyCode::Down => self.settings_field = (self.settings_field + 1).min(targets.len()),
            // Enter alone toggles (Space is deliberately inert in Settings).
            KeyCode::Enter => match self.settings_field {
                0 => self.toggle_color_blind_mode(),
                field => {
                    if let Some(target) = targets.get(field - 1) {
                        self.toggle_sound(*target);
                    }
                }
            },
            KeyCode::Left | KeyCode::Right => {
                if let Some(target) = self
                    .settings_field
                    .checked_sub(1)
                    .and_then(|i| targets.get(i))
                {
                    self.cycle_sound(*target, code == KeyCode::Right);
                }
            }
            _ => {}
        }
        self.reveal_settings_field();
    }

    fn help_form_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Enter => self.action(Action::Cancel),
            KeyCode::Up | KeyCode::Down => self.scroll_help(code == KeyCode::Down, 1),
            KeyCode::PageUp | KeyCode::PageDown => self.scroll_help(
                code == KeyCode::PageDown,
                usize::from(self.view.help.height.saturating_sub(1).max(1)),
            ),
            KeyCode::Home | KeyCode::End => self.scroll_help(code == KeyCode::End, usize::MAX),
            _ => {}
        }
    }

    fn agent_choice_key(&mut self, code: KeyCode) -> bool {
        if matches!(self.form, Some(Form::Agent { field: 1, .. }))
            && matches!(
                code,
                KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::Char(' ')
                    | KeyCode::Enter
            )
        {
            if let Some(Form::Agent {
                provider,
                provider_cursor,
                field,
                orchestrates,
                ..
            }) = &mut self.form
            {
                let choices = forms::provider_choices(orchestrates.is_some());
                let index = choices
                    .iter()
                    .position(|choice| choice == provider_cursor)
                    .unwrap_or(0);
                match code {
                    KeyCode::Left | KeyCode::Up => {
                        *provider_cursor = choices[(index + choices.len() - 1) % choices.len()];
                    }
                    KeyCode::Right | KeyCode::Down => {
                        *provider_cursor = choices[(index + 1) % choices.len()];
                    }
                    KeyCode::Char(' ') | KeyCode::Enter => {
                        *provider = Some(*provider_cursor);
                        *field = 2;
                    }
                    _ => unreachable!(),
                }
            }
            self.query_paths();
            return true;
        }
        if matches!(
            self.form,
            Some(Form::Agent {
                field: ORCHESTRATES_FIELD,
                ..
            })
        ) && matches!(
            code,
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down | KeyCode::Char(' ')
        ) {
            self.cycle_orchestrates(matches!(
                code,
                KeyCode::Right | KeyCode::Down | KeyCode::Char(' ')
            ));
            return true;
        }
        false
    }

    pub(super) fn query_paths(&mut self) {
        self.suggestions.entries.clear();
        self.suggestions.query_id += 1;
        if let Some((input, directories_only)) = self.form.as_ref().and_then(Form::path_query) {
            let query_id = self.suggestions.query_id;
            self.queue(
                BusCommand::Suggestions {
                    query_id,
                    input,
                    directories_only,
                },
                Effect::None,
            );
        }
    }

    pub(super) fn complete_path(&mut self) {
        let Some(entry) = self
            .suggestions
            .entries
            .get(self.suggestions.selected)
            .cloned()
        else {
            return;
        };
        if !entry.is_directory && matches!(self.form, Some(Form::Files(_))) {
            if let Some(room) = self.room {
                self.queue(
                    BusCommand::AttachFile(room, entry.path.display().to_string()),
                    Effect::Files(room),
                );
                self.open_room(room);
            }
            return;
        }
        if let Some(editor) = self.form.as_mut().and_then(Form::editor_mut) {
            *editor = Editor::new(format!("{}/", entry.path.display()));
        }
        // Completing a directory keeps it selected; type a prefix or press Down to browse further.
        self.suggestions.entries.clear();
        self.suggestions.query_id += 1;
    }

    fn add_agent_form(&mut self, form: Form) {
        let Form::Agent {
            name,
            provider,
            cwd,
            args,
            orchestrates,
            prompt,
            ..
        } = form
        else {
            return;
        };
        let mut missing = Vec::new();
        if name.text.trim().is_empty() {
            missing.push("Name");
        }
        if provider.is_none() {
            missing.push("Model");
        }
        if cwd.text.trim().is_empty() {
            missing.push("PWD");
        }
        if !missing.is_empty() {
            self.error = Some(match missing.as_slice() {
                [field] => format!("{field} is required."),
                [first, second] => format!("{first} and {second} are required."),
                [first, second, third] => {
                    format!("{first}, {second}, and {third} are required.")
                }
                _ => unreachable!(),
            });
            return;
        }
        if orchestrates.is_some_and(|choice| choice.0.is_none()) {
            self.error = Some(
                "Orchestrates room is required: create a work room without an orchestrator first."
                    .into(),
            );
            return;
        }
        // The MASTER form never offers Codex; this keeps the form and
        // the coordinator, which refuses it too, in step.
        if let (Some(_), Some(provider)) = (orchestrates, provider) {
            if let Err(error) = crate::bus::orchestrator::check_new_orchestrator(provider) {
                self.error = Some(error);
                return;
            }
        }
        let system_prompt = prompt.map(|prompt| prompt.editor.text);
        if system_prompt
            .as_ref()
            .is_some_and(|text| text.trim().is_empty())
        {
            self.error = Some("System prompt is required.".into());
            return;
        }
        self.error = None;
        if let Some(room) = self.room {
            let input = AddAgent {
                room,
                name: name.text,
                provider: provider.expect("validated provider"),
                cwd: cwd.text,
                extra_args: args.text,
                consent_project_hooks: false,
            };
            let command = match orchestrates.and_then(|choice| choice.0) {
                Some(room) => BusCommand::AddOrchestrator(
                    input,
                    OrchestratorSpec {
                        room,
                        system_prompt,
                    },
                ),
                None => BusCommand::AddAgent(input),
            };
            self.queue(command, Effect::None);
        }
    }

    pub(super) fn add(&mut self) {
        if self.pending.iter().any(|p| {
            matches!(
                p.command,
                BusCommand::AddAgent(_)
                    | BusCommand::AddOrchestrator(..)
                    | BusCommand::CreateRoom(_)
            )
        }) {
            return;
        }
        let Some(form) = self.form.clone() else {
            return;
        };
        match form {
            Form::Help { .. } | Form::Settings => {}
            Form::Room(editor) => {
                self.queue(BusCommand::CreateRoom(editor.text), Effect::None);
            }
            form @ Form::Agent { .. } => self.add_agent_form(form),
            Form::Files(editor) => {
                if let Some(room) = self.room {
                    self.queue(
                        BusCommand::AttachFile(room, editor.text),
                        Effect::Files(room),
                    );
                    self.open_room(room);
                }
            }
            Form::Consent {
                mut input,
                orchestrator,
                ..
            } => {
                input.consent_project_hooks = true;
                let command = match orchestrator {
                    Some(spec) => BusCommand::AddOrchestrator(input, spec),
                    None => BusCommand::AddAgent(input),
                };
                self.queue(command, Effect::None);
            }
        }
    }
}
