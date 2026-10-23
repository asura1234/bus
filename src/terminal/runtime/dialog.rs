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
    /// No coherent completed frame arrived before the bounded redraw wait.
    Redrawing,
    /// The option does not exist or the current selection is not visible.
    Unreachable,
    /// Text answering needs a free-text question or field.
    NotQuestion,
    /// The question's skip key would cancel more than this question.
    Unskippable,
    /// The free-text row already holds text someone typed.
    FreeTextDraft,
    /// Arrows moved to a free-text row scrolled out of view; no text was
    /// typed, since only the redrawn row shows whether it holds a draft.
    Focused(Vec<String>),
}

impl TerminalRuntime {
    /// Log only hashes and metadata from the exact snapshot being checked.
    pub(crate) fn trace_dialog_fingerprint(
        &self,
        dialog: Option<&crate::agents::dialog::Dialog>,
        phase: &str,
        expected_digest: Option<&str>,
        content_revision: u64,
        synchronized_output: bool,
    ) {
        crate::agents::dialog::trace_fingerprint(
            dialog,
            phase,
            expected_digest,
            content_revision,
            self.pane_id.raw(),
            synchronized_output,
        );
    }

    /// Open only the live Codex queue under the same lock as terminal updates.
    pub(crate) fn try_open_pending_codex_question(&self) -> Result<(), String> {
        let _guard = self
            .content_write_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if crate::agents::dialog::codex_question_pending(&self.terminal.visible_ansi_unwrapped()) {
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
        let (_content_write_guard, dialog) =
            match self.lock_observed_dialog(expected_digest, "choose") {
                Ok(observed) => observed,
                Err(reason) => return Ok(reason),
            };
        let Some(keys) = dialog.keys_for(option) else {
            return Ok(DialogChoice::Unreachable);
        };
        let encode = |key: &str| self.encode_dialog_key(key);
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

    /// The content lock spans a PTY chunk, not a synchronized frame. Release
    /// it while waiting so the PTY reader can complete its repaint, then keep
    /// it through parse, exact-digest check and key enqueueing.
    fn lock_observed_dialog(
        &self,
        expected_digest: &str,
        phase: &str,
    ) -> Result<(std::sync::MutexGuard<'_, ()>, crate::agents::dialog::Dialog), DialogChoice> {
        use std::time::{Duration, Instant};
        let timeout = Duration::from_millis(40);
        #[cfg(test)]
        let timeout = self.test_dialog_wait_timeout(timeout);
        let deadline = Instant::now() + timeout;
        loop {
            let guard = self
                .content_write_lock
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let synchronized = self.synchronized_output_active();
            let revision = self.content_seq();
            if synchronized || !revision.is_multiple_of(2) {
                drop(guard);
                #[cfg(test)]
                self.test_signal_dialog_redraw();
                if Instant::now() >= deadline {
                    tracing::debug!(
                        event = "bus.dialog.redrawing",
                        phase,
                        pane = self.pane_id.raw(),
                        content_revision = revision,
                        synchronized_output = synchronized,
                        "No completed dialog frame before the guarded-write deadline"
                    );
                    return Err(DialogChoice::Redrawing);
                }
                std::thread::sleep(Duration::from_millis(2));
                continue;
            }
            let dialog = crate::agents::dialog::parse(&self.terminal.visible_ansi_unwrapped());
            self.trace_dialog_fingerprint(
                dialog.as_ref(),
                phase,
                Some(expected_digest),
                revision,
                false,
            );
            return dialog
                .filter(|dialog| dialog.digest() == expected_digest)
                .map(|dialog| (guard, dialog))
                .ok_or(DialogChoice::Stale);
        }
    }

    fn encode_dialog_key(&self, key: &str) -> Vec<u8> {
        let code = match key {
            "up" => crossterm::event::KeyCode::Up,
            "down" => crossterm::event::KeyCode::Down,
            "left" => crossterm::event::KeyCode::Left,
            "right" => crossterm::event::KeyCode::Right,
            "enter" => crossterm::event::KeyCode::Enter,
            digit => crossterm::event::KeyCode::Char(digit.chars().next().unwrap_or('0')),
        };
        self.encode_terminal_key(
            crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE).into(),
        )
    }

    /// Choose the live dialog option whose label starts with `label`. The
    /// choice re-checks the dialog under the content lock, so a screen that
    /// changed in between is `Stale` and gets no keys.
    pub(crate) fn try_choose_dialog_option_labeled(
        &self,
        label: &str,
    ) -> Result<DialogChoice, String> {
        let Some(dialog) = crate::agents::dialog::parse(&self.terminal.visible_ansi_unwrapped())
        else {
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
        let (_content_write_guard, dialog) =
            match self.lock_observed_dialog(expected_digest, "answer") {
                Ok(observed) => observed,
                Err(reason) => return Ok(reason),
            };
        let Some(input) = dialog.input.as_ref() else {
            return self.answer_free_text_option(&dialog, text, skip);
        };
        if text.is_some() && !input.value.is_empty() {
            return Ok(DialogChoice::FreeTextDraft);
        }
        if skip && input.skip_key.is_none() {
            return Ok(DialogChoice::Unskippable);
        }
        let (code, modifiers, key) = if skip && input.skip_key == Some("ctrl+]") {
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

    /// Claude's option lists end with an inline text field. Arrows focus it
    /// and the text fills it in one write. Single-select fields need Enter;
    /// checkbox fields tick themselves on paste, so Enter would untick them.
    fn answer_free_text_option(
        &self,
        dialog: &crate::agents::dialog::Dialog,
        text: Option<String>,
        skip: bool,
    ) -> Result<DialogChoice, String> {
        let (Some(text), false) = (text, skip) else {
            return Ok(DialogChoice::NotQuestion);
        };
        if dialog.free_text_option().is_none() {
            return Ok(DialogChoice::NotQuestion);
        }
        if dialog.free_text_draft() {
            return Ok(DialogChoice::FreeTextDraft);
        }
        let Some(moves) = dialog.free_text_keys() else {
            return Ok(DialogChoice::Unreachable);
        };
        if !dialog.options.iter().any(|option| option.free_text) {
            self.io
                .try_send_bytes(Bytes::from(
                    moves
                        .iter()
                        .flat_map(|key| self.encode_dialog_key(key))
                        .collect::<Vec<_>>(),
                ))
                .map_err(|error| error.to_string())?;
            return Ok(DialogChoice::Focused(
                moves.into_iter().map(str::to_owned).collect(),
            ));
        }
        if !self.bracketed_paste_enabled() && text.contains(['\n', '\t']) {
            return Err("Multiline answers require bracketed paste support".into());
        }
        let mut input: Vec<u8> = moves
            .iter()
            .flat_map(|key| self.encode_dialog_key(key))
            .collect();
        input.extend_from_slice(&self.paste_payload(text));
        let mut keys: Vec<String> = moves.into_iter().map(str::to_owned).collect();
        if dialog
            .free_text_summary()
            .is_some_and(|(_, _, checkbox)| checkbox)
        {
            self.io
                .try_send_bytes(Bytes::from(input))
                .map_err(|error| error.to_string())?;
            return Ok(DialogChoice::Sent(keys));
        }
        self.io
            .queue_user_input_submission(
                Bytes::from(input),
                Bytes::from(self.encode_dialog_key("enter")),
                DIALOG_CONFIRM_DELAY,
                None,
            )
            .map_err(|error| error.to_string())?;
        keys.push("enter".into());
        Ok(DialogChoice::Sent(keys))
    }
}
