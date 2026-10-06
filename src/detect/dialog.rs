//! Numbered choice dialogs on an agent's visible screen, provider-neutral.
//!
//! Permission prompts, trust prompts and question panels from Claude Code,
//! Codex and similar TUIs all render a contiguous block of `1. ...`, `2. ...`
//! options, the selected one led by a marker such as `❯` or `›` or drawn
//! highlighted, with the question above and usually a key hint below. Only the
//! last numbered block on screen can be the live dialog; earlier ones are
//! transcript. The screen may carry ANSI styling, which is how a highlighted
//! selection is found when no marker is drawn.

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
    /// Identifies this dialog by its question and options, whichever option is
    /// selected, so a moved selection is still the same dialog.
    pub(crate) fn id(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.text.as_bytes());
        for option in &self.options {
            hasher.update(format!("\0{}\0{}", option.number, option.label));
        }
        format!("{:x}", hasher.finalize())
    }

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

    /// The keys that choose `number`: arrows from the selected option, then
    /// Enter. `None` when the option does not exist or no selection is visible.
    pub(crate) fn keys_for(&self, number: u32) -> Option<Vec<&'static str>> {
        if !self.options.iter().any(|option| option.number == number) {
            return None;
        }
        let selected = self.selected()?;
        let step = if number > selected { "down" } else { "up" };
        let mut keys = vec![step; number.abs_diff(selected) as usize];
        keys.push("enter");
        Some(keys)
    }
}

struct OptionLine {
    selected: bool,
    number: u32,
    /// Column of the number, so wrapped label lines can be recognised.
    column: usize,
    label: String,
    /// Character index of the label in its screen line, to read its style.
    label_at: usize,
    line: usize,
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

fn option_line(full: &str) -> Option<OptionLine> {
    let line = unboxed(full);
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
    // `label` is a slice of `line`, which is a slice of the screen line.
    let offset = label.as_ptr() as usize - full.as_ptr() as usize;
    Some(OptionLine {
        selected,
        number,
        column,
        label: label.to_owned(),
        label_at: full[..offset].chars().count(),
        line: 0,
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

/// The live dialog on `screen`, plain or ANSI-styled, or `None` when no choice
/// dialog is visible.
pub(crate) fn parse(screen: &str) -> Option<Dialog> {
    let (texts, styles) = styled_lines(screen);
    let lines: Vec<&str> = texts.iter().map(String::as_str).collect();
    // The last option numbered 1 starts the only block that can be live.
    let start = lines
        .iter()
        .rposition(|line| option_line(line).is_some_and(|option| option.number == 1))?;
    let mut options: Vec<OptionLine> = Vec::new();
    let mut end = start;
    for (index, line) in lines.iter().enumerate().skip(start) {
        if let Some(mut option) = option_line(line) {
            if option.number as usize != options.len() + 1 {
                break;
            }
            option.line = index;
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
    let marked = options.iter().any(|option| option.selected);
    if prompt_below || (hint.is_none() && !marked) {
        return None;
    }
    if !marked {
        let label_styles: Vec<Style> = options
            .iter()
            .map(|option| {
                styles[option.line]
                    .get(option.label_at)
                    .copied()
                    .unwrap_or_default()
            })
            .collect();
        if let Some(index) = highlighted(&label_styles) {
            options[index].selected = true;
        }
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

/// How one screen character is drawn, as far as selection highlights go.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Style {
    bold: bool,
    inverse: bool,
    fg: Option<u32>,
    bg: Option<u32>,
}

/// The option drawn unlike the others: the only inverse or filled one, or the
/// only one styled differently from options that all agree. With two options
/// the other one must be plain, or the highlight is ambiguous.
fn highlighted(styles: &[Style]) -> Option<usize> {
    let filled: Vec<usize> = (0..styles.len())
        .filter(|&index| styles[index].inverse || styles[index].bg.is_some())
        .collect();
    if !filled.is_empty() {
        return (filled.len() == 1).then(|| filled[0]);
    }
    let mut candidates = (0..styles.len()).filter(|&index| {
        let mut others = styles
            .iter()
            .enumerate()
            .filter(|(other, _)| *other != index)
            .map(|(_, style)| style);
        let Some(first) = others.next() else {
            return false;
        };
        others.all(|style| style == first)
            && *first != styles[index]
            && (styles.len() >= 3 || *first == Style::default())
    });
    let found = candidates.next();
    candidates.next().is_none().then_some(found).flatten()
}

/// Splits a screen into plain lines and the style of each of their characters,
/// following SGR sequences and skipping every other escape sequence.
fn styled_lines(screen: &str) -> (Vec<String>, Vec<Vec<Style>>) {
    let mut texts = vec![String::new()];
    let mut styles = vec![Vec::new()];
    let mut style = Style::default();
    let mut chars = screen.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.next() {
                Some('[') => {
                    let mut params = String::new();
                    for c in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&c) {
                            if c == 'm' {
                                apply_sgr(&mut style, &params);
                            }
                            break;
                        }
                        params.push(c);
                    }
                }
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\n' => {
                texts.push(String::new());
                styles.push(Vec::new());
            }
            '\r' => {}
            c => {
                texts.last_mut().unwrap().push(c);
                styles.last_mut().unwrap().push(style);
            }
        }
    }
    (texts, styles)
}

fn apply_sgr(style: &mut Style, params: &str) {
    let mut codes = params.split(';').peekable();
    if params.is_empty() {
        *style = Style::default();
    }
    while let Some(code) = codes.next() {
        // Colon form: one parameter such as `38:2::255:0:0` or `48:5:236`.
        if code.contains(':') {
            let parts: Vec<u32> = code
                .split(':')
                .filter_map(|part| part.parse().ok())
                .collect();
            let color = match parts.as_slice() {
                [_, 5, index] => Some(*index),
                [_, 2, .., r, g, b] => Some(0x100_0000 | r << 16 | g << 8 | b),
                _ => None,
            };
            match parts.first() {
                Some(38) => style.fg = color,
                Some(48) => style.bg = color,
                _ => {}
            }
            continue;
        }
        let extended = |codes: &mut std::iter::Peekable<std::str::Split<'_, char>>| {
            let mut next = || codes.next().and_then(|part| part.parse::<u32>().ok());
            match next() {
                Some(5) => next(),
                Some(2) => {
                    let (r, g, b) = (next()?, next()?, next()?);
                    Some(0x100_0000 | r << 16 | g << 8 | b)
                }
                _ => None,
            }
        };
        match code.parse::<u32>().unwrap_or(0) {
            0 => *style = Style::default(),
            1 => style.bold = true,
            22 => style.bold = false,
            7 => style.inverse = true,
            27 => style.inverse = false,
            code @ (30..=37 | 90..=97) => style.fg = Some(code),
            39 => style.fg = None,
            code @ (40..=47 | 100..=107) => style.bg = Some(code),
            49 => style.bg = None,
            38 => style.fg = extended(&mut codes),
            48 => style.bg = extended(&mut codes),
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "dialog_tests.rs"]
mod tests;
