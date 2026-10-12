use crate::terminal::runtime::TerminalRuntime;
use bytes::Bytes;

/// Gap between selection moves and the Enter that confirms them.
pub(super) const DIALOG_CONFIRM_DELAY: std::time::Duration = std::time::Duration::from_millis(150);

/// The outcome of answering a choice dialog under the content lock.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DialogChoice {
    /// The keys were queued for the dialog the caller observed.
    Sent(Vec<String>),
    /// The visible dialog no longer matches the caller's observation.
    Stale,
    /// The option does not exist or the current selection is not visible.
    Unreachable,
    /// Text answering is only available on a focused free-text question.
    NotQuestion,
}

impl TerminalRuntime {
    /// Open only the live Codex queue under the same lock as terminal updates.
    pub(crate) fn try_open_pending_codex_question(&self) -> Result<(), String> {
        let _guard = self
            .content_write_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if crate::agents::dialog::codex_question_pending(&self.terminal.visible_ansi()) {
            let key = crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Left,
                crossterm::event::KeyModifiers::SHIFT,
            );
            self.io
                .try_send_bytes(Bytes::from(self.encode_terminal_key(key.into())))
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    /// Re-parse the live choice dialog and send the keys that choose `option`
    /// while holding the lock that serializes terminal content updates, so the
    /// keys only ever reach the exact dialog the caller observed.
    pub(crate) fn try_choose_dialog_option(
        &self,
        expected_digest: &str,
        option: u32,
    ) -> Result<DialogChoice, String> {
        let _content_write_guard = match self.content_write_lock.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(dialog) = crate::agents::dialog::parse(&self.terminal.visible_ansi())
            .filter(|dialog| dialog.digest() == expected_digest)
        else {
            return Ok(DialogChoice::Stale);
        };
        let Some(keys) = dialog.keys_for(option) else {
            return Ok(DialogChoice::Unreachable);
        };
        let encode = |key: &str| {
            let code = match key {
                "up" => crossterm::event::KeyCode::Up,
                "down" => crossterm::event::KeyCode::Down,
                "enter" => crossterm::event::KeyCode::Enter,
                digit => crossterm::event::KeyCode::Char(digit.chars().next().unwrap_or('0')),
            };
            self.encode_terminal_key(
                crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE).into(),
            )
        };
        match keys.split_last() {
            // Moves then Enter: confirm only after the TUI redrew the selection.
            Some((enter, moves)) if !moves.is_empty() => {
                let moves: Vec<u8> = moves.iter().flat_map(|key| encode(key)).collect();
                self.io
                    .queue_user_input_submission(
                        Bytes::from(moves),
                        Bytes::from(encode(enter)),
                        DIALOG_CONFIRM_DELAY,
                        None,
                    )
                    .map_err(|error| error.to_string())?;
            }
            _ => self
                .io
                .try_send_bytes(Bytes::from(
                    keys.iter().flat_map(|key| encode(key)).collect::<Vec<_>>(),
                ))
                .map_err(|error| error.to_string())?,
        }
        Ok(DialogChoice::Sent(
            keys.into_iter().map(str::to_owned).collect(),
        ))
    }

    /// Choose the live dialog option whose label starts with `label`. The
    /// choice re-checks the dialog under the content lock, so a screen that
    /// changed in between is `Stale` and gets no keys.
    pub(crate) fn try_choose_dialog_option_labeled(
        &self,
        label: &str,
    ) -> Result<DialogChoice, String> {
        let Some(dialog) = crate::agents::dialog::parse(&self.terminal.visible_ansi()) else {
            return Ok(DialogChoice::Stale);
        };
        let Some(option) = dialog
            .options
            .iter()
            .find(|option| option.label.starts_with(label))
        else {
            return Ok(DialogChoice::Unreachable);
        };
        self.try_choose_dialog_option(&dialog.digest(), option.number)
    }

    pub(crate) fn try_answer_dialog(
        &self,
        expected_digest: &str,
        text: Option<String>,
        skip: bool,
    ) -> Result<DialogChoice, String> {
        crate::protocol::api::schema::AgentDialogAnswerParams::validate_answer(
            text.as_deref(),
            skip,
        )?;
        let _content_write_guard = match self.content_write_lock.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(dialog) = crate::agents::dialog::parse(&self.terminal.visible_ansi())
            .filter(|dialog| dialog.digest() == expected_digest)
        else {
            return Ok(DialogChoice::Stale);
        };
        let Some(input) = dialog.input else {
            return Ok(DialogChoice::NotQuestion);
        };
        let (code, modifiers, key) = if skip && input.skip_key == "ctrl+]" {
            (
                crossterm::event::KeyCode::Char(']'),
                crossterm::event::KeyModifiers::CONTROL,
                "ctrl+]",
            )
        } else if skip {
            (
                crossterm::event::KeyCode::Esc,
                crossterm::event::KeyModifiers::NONE,
                "esc",
            )
        } else {
            (
                crossterm::event::KeyCode::Enter,
                crossterm::event::KeyModifiers::NONE,
                "enter",
            )
        };
        let key_bytes = Bytes::from(
            self.encode_terminal_key(crossterm::event::KeyEvent::new(code, modifiers).into()),
        );
        if let Some(text) = text {
            if !self.bracketed_paste_enabled() && text.contains(['\n', '\t']) {
                return Err("Multiline answers require bracketed paste support".into());
            }
            self.io
                .queue_user_input_submission(
                    self.paste_payload(text),
                    key_bytes,
                    DIALOG_CONFIRM_DELAY,
                    None,
                )
                .map_err(|error| error.to_string())?;
        } else {
            self.io
                .try_send_bytes(key_bytes)
                .map_err(|error| error.to_string())?;
        }
        Ok(DialogChoice::Sent(vec![key.into()]))
    }
}
