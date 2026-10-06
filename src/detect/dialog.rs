//! Numbered choice dialogs on an agent's visible screen, provider-neutral.
//!
//! Permission prompts, trust prompts and question panels from Claude Code,
//! Codex and similar TUIs all render a contiguous block of `1. ...`, `2. ...`
//! options, the selected one led by a marker such as `❯` or `›`, with the
//! question above and usually a key hint below. Only the last numbered block
//! on screen can be the live dialog; earlier ones are transcript.

use sha2::{Digest, Sha256};

/// Characters that lead the selected option.
const MARKERS: &[char] = &['❯', '›', '>', '→', '▶', '▸', '➤'];
/// Words that only appear in a dialog's key hint line.
const HINT_WORDS: &[&str] = &[
    "enter", "esc", "↑", "↓", "select", "navigate", "confirm", "cancel",
];
const MAX_TITLE_LINES: usize = 12;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Dialog {
    /// The question or title above the options, blank runs collapsed.
    pub(crate) text: String,
    pub(crate) options: Vec<DialogOption>,
    /// The key hint below the options, such as `Esc to cancel`.
    pub(crate) hint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DialogOption {
    pub(crate) number: u32,
    pub(crate) label: String,
    pub(crate) selected: bool,
}

impl Dialog {
    /// Identifies this exact dialog, including which option is selected.
    pub(crate) fn digest(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.text.as_bytes());
        for option in &self.options {
            hasher.update(format!(
                "\0{}\0{}\0{}",
                option.number, option.selected, option.label
            ));
        }
        hasher.update(b"\0");
        hasher.update(self.hint.as_deref().unwrap_or_default().as_bytes());
        format!("{:x}", hasher.finalize())
    }

    pub(crate) fn selected(&self) -> Option<u32> {
        self.options
            .iter()
            .find(|option| option.selected)
            .map(|option| option.number)
    }

    /// The keys that choose `number`, or `None` when the dialog shows no way
    /// to reach it safely. A hint that advertises number keys gets the digit;
    /// otherwise the selection moves with arrows from the marked option and
    /// Enter confirms. Without a marker the current selection is unknown.
    pub(crate) fn keys_for(&self, number: u32) -> Option<Vec<&'static str>> {
        if !self.options.iter().any(|option| option.number == number) {
            return None;
        }
        if self.hint.as_deref().is_some_and(advertises_number_keys) && number <= 9 {
            return Some(vec![DIGITS[number as usize]]);
        }
        let selected = self.selected()?;
        let step = if number > selected { "down" } else { "up" };
        let mut keys = vec![step; number.abs_diff(selected) as usize];
        keys.push("enter");
        Some(keys)
    }
}

const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];

fn advertises_number_keys(hint: &str) -> bool {
    let hint = hint.to_lowercase();
    hint.contains("number") || hint.contains("1-") || hint.contains("1–")
}

struct OptionLine {
    selected: bool,
    number: u32,
    /// Column of the number, so wrapped label lines can be recognised.
    column: usize,
    label: String,
}

/// Strips terminal box borders, keeping the indentation inside them.
fn unboxed(line: &str) -> &str {
    let line = line.trim_end();
    let line = line.trim_end_matches(['│', '┃', '║']).trim_end();
    match line.trim_start().strip_prefix(['│', '┃', '║']) {
        Some(inner) => inner,
        None => line,
    }
}

fn indentation(line: &str) -> usize {
    line.chars().take_while(|c| c.is_whitespace()).count()
}

fn option_line(line: &str) -> Option<OptionLine> {
    let line = unboxed(line);
    let indent = indentation(line);
    let mut rest = line.trim_start();
    let mut selected = false;
    let mut column = indent;
    if let Some(after) = rest.strip_prefix(MARKERS) {
        selected = true;
        let gap = indentation(after);
        column += 1 + gap;
        rest = &after[after.len() - after.trim_start().len()..];
    }
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 || digits > 2 {
        return None;
    }
    let number = rest[..digits].parse().ok()?;
    let label = rest[digits..].strip_prefix(". ")?.trim();
    if label.is_empty() {
        return None;
    }
    Some(OptionLine {
        selected,
        number,
        column,
        label: label.to_owned(),
    })
}

fn is_separator(line: &str) -> bool {
    let line = line.trim();
    !line.is_empty()
        && line.chars().all(|c| {
            matches!(
                c,
                '─' | '━'
                    | '═'
                    | '╌'
                    | '-'
                    | '╭'
                    | '╮'
                    | '╰'
                    | '╯'
                    | '┌'
                    | '┐'
                    | '└'
                    | '┘'
            ) || c.is_whitespace()
        })
}

fn is_hint(line: &str) -> bool {
    let line = line.to_lowercase();
    HINT_WORDS.iter().any(|word| line.contains(word))
}

/// The live dialog on `screen`, or `None` when no choice dialog is visible.
pub(crate) fn parse(screen: &str) -> Option<Dialog> {
    let lines: Vec<&str> = screen.lines().collect();
    // The last option numbered 1 starts the only block that can be live.
    let start = lines
        .iter()
        .rposition(|line| option_line(line).is_some_and(|option| option.number == 1))?;
    let mut options: Vec<OptionLine> = Vec::new();
    let mut end = start;
    for (index, line) in lines.iter().enumerate().skip(start) {
        if let Some(option) = option_line(line) {
            if option.number as usize != options.len() + 1 {
                break;
            }
            options.push(option);
            end = index + 1;
            continue;
        }
        let text = unboxed(line);
        // Claude's question panel rules off its last option.
        if is_separator(text)
            && lines
                .get(index + 1)
                .and_then(|next| option_line(next))
                .is_some_and(|next| next.number as usize == options.len() + 1)
        {
            continue;
        }
        let Some(last) = options.last_mut() else {
            break;
        };
        if text.trim().is_empty() || indentation(text) <= last.column || is_separator(text) {
            break;
        }
        // A wrapped label continues deeper than its number.
        last.label.push(' ');
        last.label.push_str(text.trim());
        end = index + 1;
    }
    if options.len() < 2 || options.iter().filter(|option| option.selected).count() > 1 {
        return None;
    }
    let after: Vec<&str> = lines[end..]
        .iter()
        .map(|line| unboxed(line).trim())
        .collect();
    let hint = after
        .iter()
        .filter(|line| !line.is_empty() && !is_separator(line))
        .take(2)
        .find(|line| is_hint(line))
        .map(|line| (*line).to_owned());
    // A live input prompt below means the list is transcript: dialogs replace
    // the composer. Without a key hint, the list also needs a marked option.
    let prompt_below = after
        .iter()
        .any(|line| line.starts_with(MARKERS) && option_line(line).is_none());
    if prompt_below || (hint.is_none() && !options.iter().any(|option| option.selected)) {
        return None;
    }
    Some(Dialog {
        text: title(&lines[..start]),
        options: options
            .into_iter()
            .map(|option| DialogOption {
                number: option.number,
                label: option.label,
                selected: option.selected,
            })
            .collect(),
        hint,
    })
}

/// The question above the options, up to a separator or a double blank line.
fn title(above: &[&str]) -> String {
    let mut collected = Vec::new();
    let mut blanks = 0;
    for line in above.iter().rev() {
        let text = unboxed(line).trim();
        if is_separator(text) {
            break;
        }
        if text.is_empty() {
            blanks += 1;
            if blanks == 2 && !collected.is_empty() {
                break;
            }
            continue;
        }
        if blanks > 0 && !collected.is_empty() {
            collected.push("");
        }
        blanks = 0;
        collected.push(text);
        if collected.iter().filter(|line| !line.is_empty()).count() == MAX_TITLE_LINES {
            break;
        }
    }
    collected.reverse();
    collected.join("\n")
}

#[cfg(test)]
#[path = "dialog_tests.rs"]
mod tests;
