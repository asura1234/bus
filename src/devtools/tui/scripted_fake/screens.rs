//! Screen-shaped scripted prompts. Answers are checked; none execute commands.
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use serde::Deserialize;
use serde_json::json;
use std::io::{self, Write};
use std::path::Path;

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum ScreenInput {
    Menu {
        labels: Vec<String>,
        answer: usize,
        #[serde(default)]
        selected: usize,
        #[serde(default = "default_marker")]
        marker: String,
        #[serde(default = "default_numbered")]
        numbered: bool,
    },
    Number {
        answer: String,
    },
    Text {
        answer: String,
    },
    Key {
        answer: String,
    },
}

fn default_marker() -> String {
    "›".into()
}

fn default_numbered() -> bool {
    true
}

impl ScreenInput {
    pub(super) fn validate(&self, screen: &str) -> Result<(), String> {
        match self {
            Self::Menu {
                labels,
                answer,
                selected,
                ..
            } => {
                if labels.is_empty()
                    || labels.iter().any(|label| label.trim().is_empty())
                    || *answer >= labels.len()
                    || *selected >= labels.len()
                    || !screen.contains("{{options}}")
                {
                    return Err(
                        "Fake menu needs labels, valid zero-based indexes and {{options}}".into(),
                    );
                }
            }
            Self::Number { answer }
                if answer.len() != 1 || !answer.bytes().all(|b| b.is_ascii_digit()) =>
            {
                return Err("Fake number answer must be one digit".into());
            }
            Self::Key { answer } if crate::utils::config::parse_key_combo(answer).is_none() => {
                return Err(format!("Unsupported fake answer key {answer:?}"));
            }
            _ => {}
        }
        Ok(())
    }
}

pub(super) struct Interaction {
    input: ScreenInput,
    draft: String,
}

impl Interaction {
    pub(super) fn new(input: ScreenInput) -> Self {
        Self {
            input,
            draft: String::new(),
        }
    }

    pub(super) fn render(&self, screen: &str) -> String {
        let options = match &self.input {
            ScreenInput::Menu {
                labels,
                selected,
                marker,
                numbered,
                ..
            } => labels
                .iter()
                .enumerate()
                .map(|(index, label)| {
                    let mark = if index == *selected {
                        marker.as_str()
                    } else {
                        " "
                    };
                    let number = if *numbered {
                        format!("{}. ", index + 1)
                    } else {
                        String::new()
                    };
                    format!("{mark} {number}{label}")
                })
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };
        screen
            .replace("{{options}}", &options)
            .replace("{{answer}}", &self.draft)
    }

    /// None keeps the prompt open; Some records the exact completed answer.
    pub(super) fn apply(&mut self, event: Event) -> Option<(String, bool)> {
        let event = match event {
            Event::Key(key) if key.kind == KeyEventKind::Release => return None,
            event => event,
        };
        match &mut self.input {
            ScreenInput::Menu {
                labels,
                selected,
                answer,
                ..
            } => {
                let Event::Key(key) = event else { return None };
                match key.code {
                    KeyCode::Up => *selected = selected.saturating_sub(1),
                    KeyCode::Down => *selected = (*selected + 1).min(labels.len() - 1),
                    KeyCode::Enter => {
                        return Some((labels[*selected].clone(), *selected == *answer))
                    }
                    _ => {}
                }
            }
            ScreenInput::Text { answer } => match event {
                Event::Paste(text) => self.draft.push_str(&text),
                Event::Key(key) => match key.code {
                    KeyCode::Enter => return Some((self.draft.clone(), self.draft == *answer)),
                    KeyCode::Backspace => {
                        self.draft.pop();
                    }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.draft.clear()
                    }
                    KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.draft.push(c)
                    }
                    _ => {}
                },
                _ => {}
            },
            ScreenInput::Number { answer } => {
                if let Event::Key(key) = event {
                    if let KeyCode::Char(c) = key.code {
                        return Some((c.to_string(), c.to_string() == *answer));
                    }
                }
            }
            ScreenInput::Key { answer } => {
                if let Event::Key(key) = event {
                    let matches = crate::utils::config::parse_key_combo(answer).is_some_and(
                        |(code, modifiers)| key.code == code && key.modifiers == modifiers,
                    );
                    return Some((
                        if matches {
                            answer.clone()
                        } else {
                            format!("{:?}", key)
                        },
                        matches,
                    ));
                }
            }
        }
        None
    }
}

pub(super) fn run(
    id: &str,
    screen: &str,
    input: ScreenInput,
    spool: &Path,
) -> Result<String, String> {
    let mut interaction = Interaction::new(input);
    write!(io::stdout(), "\x1b[?1049h").map_err(|e| e.to_string())?;
    let alternate = AlternateScreen;
    let result = loop {
        let rendered = interaction.render(screen).replace('\n', "\r\n");
        write!(io::stdout(), "\x1b[H\x1b[J{rendered}").map_err(|e| e.to_string())?;
        io::stdout().flush().map_err(|e| e.to_string())?;
        if let Some(result) = interaction.apply(event::read().map_err(|e| e.to_string())?) {
            break result;
        }
    };
    drop(alternate);
    let mut audit = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(spool.join("fake-dialog-results.jsonl"))
        .map_err(|e| e.to_string())?;
    writeln!(
        audit,
        "{}",
        json!({"id":id,"answer":result.0,"passed":result.1})
    )
    .map_err(|e| e.to_string())?;
    if result.1 {
        Ok(result.0)
    } else {
        Err(format!("Dialog {id} received the wrong answer"))
    }
}

struct AlternateScreen;

impl Drop for AlternateScreen {
    fn drop(&mut self) {
        let _ = write!(io::stdout(), "\x1b[?1049l");
        let _ = io::stdout().flush();
    }
}
