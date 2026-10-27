//! Native draft mechanisms used by the scratch provider CLIs.
use super::{kill_line, FakeAgent, Input, Typed};
use std::io::Write;
use std::process::Command;

impl FakeAgent {
    /// Applies one input to the draft, redrawing it.
    pub(super) fn apply(&mut self, input: Input) -> Option<Typed> {
        match input {
            Input::Text(text) => self.draft.push_str(&text),
            Input::Paste(text) => {
                self.draft.push_str(&text);
                self.collapsed_paste = true;
            }
            // Nothing to erase: nothing to redraw.
            Input::Backspace => {
                self.draft.pop()?;
            }
            Input::KillLine => {
                if self.collapsed_paste {
                    self.draft.clear();
                } else {
                    kill_line(&mut self.draft);
                }
            }
            Input::Stash if !self.live_turn() => self.stash(),
            Input::Editor if self.live_turn() => self.edit_draft(),
            Input::Stash | Input::Editor => return None,
            Input::Enter if !self.draft.trim().is_empty() => {
                let draft = std::mem::take(&mut self.draft);
                self.collapsed_paste = false;
                let (prompt, images) = self.lift(&draft);
                self.images += images;
                if let Some((draft, collapsed)) = self.stashed.take() {
                    self.draft = draft;
                    self.collapsed_paste = collapsed;
                }
                return Some(Typed::Submit(prompt));
            }
            Input::Enter => return None,
            Input::Escape if self.working.is_some() && !self.cursor() => return Some(Typed::Stop),
            Input::Escape => return None,
            Input::Interrupt if self.working.is_some() && self.cursor() => {
                return Some(Typed::Stop)
            }
            Input::Interrupt => return Some(Typed::Exit),
        }
        if self.draft.is_empty() {
            self.collapsed_paste = false;
        }
        self.redraw();
        None
    }

    pub(super) fn stash(&mut self) {
        if self.draft.trim().is_empty() {
            if let Some((draft, collapsed)) = self.stashed.take() {
                self.draft = draft;
                self.collapsed_paste = collapsed;
            }
        } else {
            self.stashed = Some((std::mem::take(&mut self.draft), self.collapsed_paste));
            self.collapsed_paste = false;
        }
    }

    pub(super) fn shown_draft(&self) -> String {
        let (shown, images) = self.lift(&self.draft);
        if !self.collapsed_paste || self.draft.is_empty() {
            return shown;
        }
        if self.codex() {
            return format!("[Pasted Content {} chars]", self.draft.chars().count());
        }
        let (chips, text) = if images > 0 {
            let end =
                crate::agents::providers::claude_code::hooks::claude_image_placeholders(&shown).0;
            (&shown[..end], &shown[end..])
        } else {
            ("", shown.as_str())
        };
        let newlines = text.matches('\n').count();
        let lines = if self.cursor() {
            newlines + 1
        } else {
            newlines
        };
        format!(
            "{chips}[Pasted text #{} +{lines} lines]",
            self.images + images + 1
        )
    }

    /// The installed native CLIs hand the exact complete draft to VISUAL,
    /// including whitespace and offscreen lines. Only their readback trim
    /// differs: Codex trims its end, while Cursor removes one final newline.
    pub(super) fn edit_draft(&mut self) {
        let Some(editor) = std::env::var("VISUAL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| std::env::var("EDITOR").ok())
        else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("bus-fake-editor-{}", super::pseudo_uuid()));
        if std::fs::create_dir(&dir).is_err() {
            return;
        }
        let path = dir.join("prompt.md");
        let result = (|| {
            std::fs::write(&path, &self.draft)?;
            let _ = std::io::stdout().flush();
            let status = Command::new("sh")
                .args(["-c", &format!("{editor} \"$1\""), "fake-editor"])
                .arg(&path)
                .status()?;
            if status.success() {
                let text = std::fs::read_to_string(&path)?;
                self.draft = editor_readback(&text, self.codex()).to_owned();
                self.collapsed_paste = false;
            }
            std::io::Result::Ok(())
        })();
        if result.is_ok() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

fn editor_readback(text: &str, codex: bool) -> &str {
    if codex {
        text.trim_end()
    } else {
        text.strip_suffix("\r\n")
            .or_else(|| text.strip_suffix(['\n', '\r']))
            .unwrap_or(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devtools::tui::fake_agents::{drain_input, Input, Typed};
    use serde_json::Value;

    #[test]
    fn native_stash_restores_full_rich_draft_after_next_submit() {
        let mut agent = FakeAgent::new("claude", Value::Null);
        let rich = format!(
            "\"/tmp/shot.png\"\n{}",
            "keep exact pasted line\n".repeat(12)
        );
        agent.draft = rich.clone();
        agent.collapsed_paste = true;
        let shown = agent.shown_draft();
        assert!(shown.starts_with("[Image #1][Pasted text #2 +"));
        agent.apply(Input::Stash);
        assert!(agent.draft.is_empty());
        agent.apply(Input::Text("Bus delivery".into()));
        assert!(
            matches!(agent.apply(Input::Enter), Some(Typed::Submit(text)) if text == "Bus delivery")
        );
        assert_eq!(agent.draft, rich);
        assert_eq!(agent.shown_draft(), shown);
        assert!(agent.stashed.is_none());
    }

    #[test]
    fn editor_sentinel_preserves_every_trailing_character_for_each_provider() {
        let original = "  leading\tspaces\n中 文\n\n\t  \n";
        for codex in [true, false] {
            let marked = format!("{original}__BUS_END__");
            assert_eq!(editor_readback(&marked, codex), marked);
            let mut restored = editor_readback(&marked, codex).to_owned();
            for _ in "__BUS_END__".chars() {
                restored.pop();
            }
            assert_eq!(restored, original);
        }
        assert_eq!(editor_readback("draft \t\n", true), "draft");
        assert_eq!(editor_readback("draft \t\n\n", false), "draft \t\n");
    }

    #[test]
    fn large_pastes_remain_opaque_but_typed_multiline_text_is_plain() {
        let text = "not collapsed when typed\n".repeat(12);
        let mut pasted = format!("\x1b[200~{text}\x1b[201~\x13\x07").into_bytes();
        assert_eq!(
            drain_input(&mut pasted),
            vec![Input::Paste(text.clone()), Input::Stash, Input::Editor]
        );
        let mut typed = text.clone().into_bytes();
        assert_eq!(drain_input(&mut typed), vec![Input::Text(text)]);
    }
}
