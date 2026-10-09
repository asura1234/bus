//! Choice dialogs and focused text questions on an agent's visible screen.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DialogKind {
    Choice,
    Question,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct QuestionInput {
    /// Included in the digest so a human's edit invalidates an observation.
    pub(crate) value: String,
    pub(crate) skip_key: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Dialog {
    pub(crate) kind: DialogKind,
    pub(crate) input: Option<QuestionInput>,
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
        hasher.update(format!("\0{:?}", self.kind));
        if let Some(input) = &self.input {
            hasher.update(input.skip_key.as_bytes());
        }
        for option in &self.options {
            hasher.update(format!("\0{}\0{}", option.number, option.label));
        }
        format!("{:x}", hasher.finalize())
    }

    /// Identifies this exact dialog, including which option is selected.
    pub(crate) fn digest(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.id().as_bytes());
        if let Some(input) = &self.input {
            hasher.update(input.value.as_bytes());
        }
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
    let after_dot = rest[digits..].strip_prefix('.')?;
    // `1.Yes` 不是选项；`1.` 或 `1. ` 是选项正文被折到下一行。
    let wrapped = after_dot.trim().is_empty();
    if !wrapped && !after_dot.starts_with([' ', '\t']) {
        return None;
    }
    let label = after_dot.trim();
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
    numbered(&lines, &styles)
        .or_else(|| unnumbered(&lines))
        .or_else(|| text_question(&lines))
}

/// Only the live, collapsed queue beside an empty composer can be opened.
/// This is deliberately separate from parse: a pending async question can coexist
/// with Working, and status polling must neither open it nor invent a selection.
pub(crate) fn codex_question_pending(screen: &str) -> bool {
    let (texts, _) = styled_lines(screen);
    let lines: Vec<_> = texts.iter().map(|line| unboxed(line).trim()).collect();
    let Some(header) = lines
        .iter()
        .rposition(|line| line.trim_start_matches(['•', '◦']).trim() == "Queued follow-up inputs")
    else {
        return false;
    };
    let after: Vec<_> = lines[header + 1..]
        .iter()
        .copied()
        .filter(|line| !line.is_empty())
        .collect();
    let [count, hint, tail @ ..] = after.as_slice() else {
        return false;
    };
    let count = count.split(" · ").next().unwrap_or_default();
    let words: Vec<_> = count.split_whitespace().collect();
    if !matches!(words.as_slice(), ["?", n, "question" | "questions"] if n.parse::<u32>().is_ok_and(|n| n > 0))
        || !matches!(
            hint.to_lowercase().as_str(),
            "⇧← to answer" | "shift+left to answer" | "shift+← to answer"
        )
    {
        return false;
    }
    let empty_composer = |line: &str| {
        line.strip_prefix('›').is_some_and(|body| {
            matches!(
                body.trim(),
                "" | "Ask Codex to do anything" | "Use /skills to list available skills"
            )
        })
    };
    if tail.is_empty() {
        return lines[..header]
            .iter()
            .rfind(|line| line.starts_with('›'))
            .is_some_and(|line| empty_composer(line));
    }
    empty_composer(tail[0])
        && (tail.len() == 1
            || (tail.len() == 3
                && tail[2] == "? for shortcuts"
                && !tail[1].starts_with(['•', '›'])))
}

fn codex_text_footer(line: &str) -> bool {
    let lower = unboxed(line).trim().to_lowercase();
    lower.contains("enter")
        && lower.contains("submit")
        && lower.contains("skip")
        && (lower.contains("ctrl+]") || lower.contains("⌃]"))
        && lower.contains("main prompt")
}

/// A live footer must be last: a composer or activity below makes it transcript.
fn text_question(lines: &[&str]) -> Option<Dialog> {
    let footer = lines.iter().rposition(|line| {
        let text = unboxed(line).trim();
        !text.is_empty() && !is_separator(text)
    })?;
    let hint = unboxed(lines[footer]).trim();
    if codex_text_footer(hint) {
        let header = lines[..footer].iter().rposition(|line| {
            unboxed(line)
                .trim()
                .trim_start_matches(['•', '◦'])
                .trim()
                .eq_ignore_ascii_case("Queued follow-up inputs")
        })?;
        let input =
            (header + 1..footer).rfind(|&index| !unboxed(lines[index]).trim().is_empty())?;
        let mut input_start = input;
        while input_start > header + 1 && !unboxed(lines[input_start - 1]).trim().is_empty() {
            input_start -= 1;
        }
        let value = lines[input_start..=input]
            .iter()
            .map(|line| unboxed(line).trim())
            .collect::<Vec<_>>()
            .join("\n");
        let question = lines[header + 1..input_start]
            .iter()
            .map(|line| unboxed(line).trim())
            .filter(|line| {
                let words: Vec<_> = line.split_whitespace().collect();
                !line.is_empty()
                    && !matches!(words.as_slice(), [n, "of", m]
                        if n.parse::<u32>().is_ok() && m.parse::<u32>().is_ok())
            })
            .collect::<Vec<_>>()
            .join("\n");
        return question_dialog(question, hint, &value, "Type your answer", "ctrl+]");
    }
    // Cursor's Other input is editable only while that checkbox row is focused.
    if hint == "↑/↓ option · ←/→ question · Space select · Enter next/submit · Esc to skip"
    {
        let active = lines[..footer].iter().rposition(|line| {
            marked_label(line).is_some_and(|(_, label)| {
                label.starts_with("[ ] Other:") || label.starts_with("[x] Other:")
            })
        })?;
        let (_, label) = marked_label(lines[active])?;
        let value = std::iter::once(label.split_once("Other:")?.1.trim())
            .chain(
                lines[active + 1..footer]
                    .iter()
                    .map(|line| unboxed(line).trim())
                    .filter(|line| !line.is_empty()),
            )
            .collect::<Vec<_>>()
            .join("\n");
        let question_at = lines[..active]
            .iter()
            .rposition(|line| option_line(line).is_some())?;
        let first = option_line(lines[question_at])?.label;
        let question = std::iter::once(first.as_str())
            .chain(
                lines[question_at + 1..active]
                    .iter()
                    .map(|line| unboxed(line).trim())
                    .take_while(|line| !line.trim_start_matches(MARKERS).trim().starts_with('['))
                    .filter(|line| !line.is_empty()),
            )
            .collect::<Vec<_>>()
            .join("\n");
        return question_dialog(question, hint, &value, "(type to answer)", "esc");
    }
    None
}

fn question_dialog(
    text: String,
    hint: &str,
    value: &str,
    placeholder: &str,
    skip_key: &'static str,
) -> Option<Dialog> {
    if text.is_empty() {
        return None;
    }
    Some(Dialog {
        kind: DialogKind::Question,
        input: Some(QuestionInput {
            value: if value == placeholder {
                String::new()
            } else {
                value.to_owned()
            },
            skip_key,
        }),
        text,
        options: Vec::new(),
        hint: Some(hint.to_owned()),
    })
}

/// A selected option without a number, such as `❯ No, exit`.
fn marked_label(line: &str) -> Option<(usize, &str)> {
    let line = unboxed(line);
    let after = line.trim_start().strip_prefix(MARKERS)?;
    let label = after.trim_start();
    if label.is_empty() || option_line(line).is_some() || after.len() == label.len() {
        return None;
    }
    Some((indentation(line) + 1 + indentation(after), label.trim()))
}

/// Claude Code's folder trust prompt draws its options without numbers:
/// `❯ No, exit` above `  Yes, I trust this folder`, then an `Enter to ...`
/// hint. Only the last marked line can be live, its unmarked siblings start in
/// the same column, and the hint must follow, which a composer never has.
/// Options are numbered top to bottom.
fn unnumbered(lines: &[&str]) -> Option<Dialog> {
    let marked = lines
        .iter()
        .rposition(|line| unboxed(line).trim_start().starts_with(MARKERS))?;
    let (column, _) = marked_label(lines[marked])?;
    let sibling = |line: &str| {
        let text = unboxed(line);
        !text.trim().is_empty()
            && indentation(text) == column
            && !is_separator(text)
            && !is_hint(text)
    };
    let mut start = marked;
    while start > 0 && sibling(lines[start - 1]) {
        start -= 1;
    }
    let mut end = marked + 1;
    while end < lines.len() && sibling(lines[end]) {
        end += 1;
    }
    let hint = lines[end..]
        .iter()
        .map(|line| unboxed(line).trim())
        .find(|line| !line.is_empty())?;
    if end - start < 2 || !hint.to_lowercase().starts_with("enter to") {
        return None;
    }
    Some(Dialog {
        kind: DialogKind::Choice,
        input: None,
        text: title(&lines[..start]),
        options: (start..end)
            .map(|index| DialogOption {
                number: (index - start + 1) as u32,
                label: match marked_label(lines[index]) {
                    Some((_, label)) => label.to_owned(),
                    None => unboxed(lines[index]).trim().to_owned(),
                },
                selected: index == marked,
            })
            .collect(),
        hint: Some(hint.to_owned()),
    })
}

/// The last block of `1. ...`, `2. ...` options, when it is a live dialog.
fn numbered(lines: &[&str], styles: &[Vec<Style>]) -> Option<Dialog> {
    // The last option numbered 1 starts the only block that can be live.
    let start = lines
        .iter()
        .rposition(|line| option_line(line).is_some_and(|option| option.number == 1))?;
    let (mut options, end) = numbered_options(lines, start);
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
    let prompt_below = after.iter().any(|line| {
        (line.starts_with(MARKERS) && option_line(line).is_none())
            || line
                .trim_start_matches(['•', '◦'])
                .trim()
                .eq_ignore_ascii_case("Queued follow-up inputs")
    });
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
    // Codex's last numbered row is its editable Other field. Typing replaces
    // the placeholder itself; Enter on an empty field leaves the form open.
    if let Some(hint) = hint.as_deref().filter(|hint| codex_text_footer(hint)) {
        if lines[..start].iter().any(|line| {
            unboxed(line).trim().trim_start_matches(['•', '◦']).trim() == "Queued follow-up inputs"
        }) {
            if let Some(last) = options.last().filter(|option| option.selected) {
                return question_dialog(
                    title(&lines[..start]),
                    hint,
                    &last.label,
                    "Other",
                    "ctrl+]",
                );
            }
        }
    }
    // Claude's custom answer is editable at the resting selection, before
    // Enter. Its editor hint only appears while that text field is focused.
    if let Some(hint) = hint
        .as_deref()
        .filter(|hint| hint.contains("ctrl+g to edit") && hint.contains("Esc to cancel"))
    {
        if let Some(selected) = options.iter().find(|option| option.selected) {
            if selected.number as usize == options.len() - 1
                && options
                    .last()
                    .is_some_and(|option| option.label == "Chat about this")
            {
                let question = title(&lines[..start])
                    .lines()
                    .filter(|line| !line.starts_with('☐'))
                    .collect::<Vec<_>>()
                    .join("\n")
                    .trim()
                    .to_owned();
                return question_dialog(question, hint, &selected.label, "Type something.", "esc");
            }
        }
    }
    Some(Dialog {
        kind: DialogKind::Choice,
        input: None,
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

/// Collects consecutive numbered options, including their wrapped labels.
fn numbered_options(lines: &[&str], start: usize) -> (Vec<OptionLine>, usize) {
    let mut options: Vec<OptionLine> = Vec::new();
    let mut end = start;
    for (index, line) in lines.iter().enumerate().skip(start) {
        if let Some(mut option) = option_line(line) {
            if option.number as usize != options.len() + 1 {
                break;
            }
            // Codex 把放不下的选项正文折到下一行，编号行上只剩 `› 1.`。
            if option.label.is_empty()
                && !lines.get(index + 1).is_some_and(|next| {
                    let text = unboxed(next);
                    !text.trim().is_empty()
                        && option_line(next).is_none()
                        && !is_separator(text)
                        && indentation(text) > option.column
                })
            {
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
        if !last.label.is_empty() {
            last.label.push(' ');
        }
        last.label.push_str(text.trim());
        end = index + 1;
    }
    (options, end)
}

/// The question above the options, up to a separator or a double blank line.
fn title(above: &[&str]) -> String {
    // Approval notices must retain every command line and the Reason above it.
    let limit = if above
        .iter()
        .any(|line| unboxed(line).trim().starts_with("$ "))
    {
        usize::MAX
    } else {
        MAX_TITLE_LINES
    };
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
        if collected.iter().filter(|line| !line.is_empty()).count() == limit {
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
    let mut texts = Vec::new();
    let mut styles = Vec::new();
    // Keep the current line directly, so even an empty screen has a line and
    // character appends never depend on a fallible last-element lookup.
    let mut text = String::new();
    let mut line_styles = Vec::new();
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
                texts.push(std::mem::take(&mut text));
                styles.push(std::mem::take(&mut line_styles));
            }
            '\r' => {}
            c => {
                text.push(c);
                line_styles.push(style);
            }
        }
    }
    texts.push(text);
    styles.push(line_styles);
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
#[path = "tests/dialog_test.rs"]
mod tests;
