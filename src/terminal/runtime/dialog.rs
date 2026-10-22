use crate::terminal::runtime::TerminalRuntime;
use bytes::Bytes;

/// Gap between selection moves and the Enter that confirms them.
pub(super) const DIALOG_CONFIRM_DELAY: std::time::Duration = std::time::Duration::from_millis(150);

#[cfg(test)]
mod grok_tests {
    use super::*;

    const PERMISSION: &str = include_str!("../../../tests/fixtures/grok-dialog/permission.txt");

    #[tokio::test]
    async fn grok_c_changed_scope_is_stale_under_runtime_content_lock() {
        let (runtime, mut writes) =
            crate::terminal::TerminalRuntime::test_with_channel_capacity(100, 24, 16);
        let draw = |screen: &str| {
            runtime.test_process_pty_bytes(
                format!("\x1b[2J\x1b[H{}", screen.replace('\n', "\r\n")).as_bytes(),
            )
        };
        draw(PERMISSION);
        let (snapshot, _) = runtime.visible_ansi_snapshot_with_seq().unwrap();
        let original = crate::agents::dialog::parse(&snapshot).expect("emulated card");
        draw(&PERMISSION.replace("this command", "all commands"));
        assert_eq!(
            runtime
                .try_choose_dialog_option(&original.digest(), 1)
                .unwrap(),
            crate::terminal::runtime::DialogChoice::Stale
        );
        assert!(writes.try_recv().is_err(), "stale approval received keys");
    }

    #[tokio::test]
    async fn grok_g_dismiss_exits_free_text_before_shift_x() {
        for kitty in [false, true] {
            let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(120, 40, 8);
            if kitty {
                runtime.test_process_pty_bytes(b"\x1b[>9u");
            }
            runtime.test_process_pty_bytes(
                include_str!("../../../tests/fixtures/grok-dialog/free-text.txt")
                    .replace('\n', "\r\n")
                    .as_bytes(),
            );
            let (screen, _) = runtime.visible_ansi_snapshot_with_seq().unwrap();
            let dialog = crate::agents::dialog::parse(&screen).expect("joined Grok editor fixture");
            while writes.try_recv().is_ok() {}
            assert_eq!(
                runtime
                    .try_answer_dialog(&dialog.digest(), None, true)
                    .unwrap(),
                DialogChoice::Sent(vec!["esc".into(), "shift+x".into()])
            );
            let first = tokio::time::timeout(std::time::Duration::from_secs(2), writes.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(
                first == b"\x1b"[..] || first == b"\x1b[27;1u"[..] || first == b"\x1b[27u"[..],
                "{first:?}"
            );
            let second = tokio::time::timeout(std::time::Duration::from_secs(2), writes.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                second.as_ref(),
                if kitty {
                    &b"\x1b[120;2u"[..]
                } else {
                    &b"X"[..]
                }
            );
        }
    }
}

/// The outcome of answering a choice dialog under the content lock.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DialogChoice {
    /// The keys were queued for the dialog the caller observed.
    Sent(Vec<String>),
    /// The visible dialog no longer matches the caller's observation.
    Stale,
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
        let _content_write_guard = match self.content_write_lock.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(dialog) = crate::agents::dialog::parse(&self.terminal.visible_ansi_unwrapped())
            .filter(|dialog| dialog.digest() == expected_digest)
        else {
            return Ok(DialogChoice::Stale);
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
        let _content_write_guard = match self.content_write_lock.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(dialog) = crate::agents::dialog::parse(&self.terminal.visible_ansi_unwrapped())
            .filter(|dialog| dialog.digest() == expected_digest)
        else {
            return Ok(DialogChoice::Stale);
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
        if skip && input.skip_key == Some("shift+x") {
            use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
            self.io
                .queue_user_input_submission(
                    Bytes::from(self.encode_terminal_key(
                        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE).into(),
                    )),
                    Bytes::from(self.encode_terminal_key(
                        KeyEvent::new(KeyCode::Char('X'), KeyModifiers::SHIFT).into(),
                    )),
                    DIALOG_CONFIRM_DELAY,
                    None,
                )
                .map_err(|error| error.to_string())?;
            return Ok(DialogChoice::Sent(vec!["esc".into(), "shift+x".into()]));
        }
        if skip && !matches!(input.skip_key, Some("ctrl+]" | "esc")) {
            return Ok(DialogChoice::Unreachable);
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
