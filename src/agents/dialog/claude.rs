//! Claude AskUserQuestion panels have unnumbered buttons and horizontal tabs.
use super::{
    highlighted, indentation, is_separator, marked_label, option_line, question_dialog, title,
    unboxed, Dialog, DialogKind, DialogOption, DialogTab, OptionLine, Style, CHAT_LABEL,
};

pub(super) fn parse(lines: &[&str], styles: &[Vec<Style>]) -> Option<Dialog> {
    let start = lines
        .iter()
        .rposition(|line| option_line(line).is_some_and(|option| option.number == 1))?;
    let header = lines[..start].iter().rposition(|line| {
        let text = unboxed(line).trim();
        text.contains('☐') || text.contains('☑') || text.contains("✔ Submit")
    })?;
    let tabbed = unboxed(lines[header]).trim().starts_with('←')
        && unboxed(lines[header]).trim().ends_with('→');
    if !tabbed && !option_line(lines[start])?.label.starts_with('[') {
        return None;
    }
    let footer = lines
        .iter()
        .rposition(|line| unboxed(line).trim().starts_with("Enter to"))?;
    let hint = lines[footer..]
        .iter()
        .map(|line| unboxed(line).trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if !hint.ends_with("Esc to cancel")
        || !(hint.contains("navigate") || hint.contains("ctrl+g to edit"))
    {
        return None;
    }
    let mut rows = collect_rows(lines, start, footer)?;
    if !rows.iter().any(|row| row.selected) {
        let row_styles: Vec<_> = rows
            .iter()
            .map(|row| {
                styles[row.line]
                    .get(row.label_at)
                    .copied()
                    .unwrap_or_default()
            })
            .collect();
        if let Some(index) = highlighted(&row_styles) {
            rows[index].selected = true;
        }
    }
    if rows.iter().filter(|row| row.selected).count() != 1 {
        return None;
    }
    if hint.contains("ctrl+g to edit") {
        let selected = rows.iter().find(|row| row.selected)?;
        let mut dialog = text_question(&lines[header..start], &hint, &selected.label)?;
        dialog.fingerprint_context = format!("{:?}", styles[header]);
        return Some(dialog);
    }
    let free_text = free_text_row(&rows);
    let mut options: Vec<_> = rows
        .into_iter()
        .enumerate()
        .map(|(index, row)| DialogOption {
            number: index as u32 + 1,
            identity_label: checkbox_label(&row.label).map(str::to_owned),
            free_text: Some(row.number) == free_text,
            label: row.label,
            selected: row.selected,
            navigation_key: None,
        })
        .collect();
    if tabbed {
        for (label, key) in [("Previous question", "left"), ("Next question", "right")] {
            options.push(DialogOption {
                number: options.len() as u32 + 1,
                label: label.to_owned(),
                selected: false,
                navigation_key: Some(key),
                identity_label: None,
                free_text: false,
            });
        }
    }
    Some(Dialog {
        kind: DialogKind::Choice,
        input: None,
        text: title(&lines[header..start]),
        options,
        hint: Some(hint),
        fingerprint_context: format!("{:?}", styles[header]),
        tabs: Vec::new(),
    })
}

/// Claude's free-text field sits right above `Chat about this`; a scrolled
/// list can hide it, leaving a gap in the numbers.
/// A multi-select form's field is the `[ ] Type something` checkbox instead.
pub(super) fn free_text_row(options: &[OptionLine]) -> Option<u32> {
    let chat = options.last().filter(|chat| chat.label == CHAT_LABEL)?;
    if let Some(row) = options.iter().find(|row| {
        checkbox_label(&row.label)
            .is_some_and(|label| label.trim_end_matches('.') == "Type something")
    }) {
        return Some(row.number);
    }
    match options {
        [.., field, _]
            if field.number + 1 == chat.number && checkbox_label(&field.label).is_none() =>
        {
            Some(field.number)
        }
        _ => None,
    }
}

/// The tabs of a multi-question form, `←  ☒ Fresh files  ☐ Helper home  ✔ Submit  →`:
/// `☒` marks an answered question and the active tab is drawn filled.
pub(super) fn tabs(lines: &[&str], styles: &[Vec<Style>]) -> Vec<DialogTab> {
    let Some(index) = lines.iter().rposition(|line| {
        let text = unboxed(line).trim();
        text.starts_with('←') && text.ends_with('→') && text.contains("✔ Submit")
    }) else {
        return Vec::new();
    };
    let glyph = |c: char| matches!(c, '☐' | '☒' | '☑' | '✔');
    let line = lines[index];
    line.char_indices()
        .filter(|(_, c)| glyph(*c))
        .map(|(at, mark)| {
            let rest = &line[at + mark.len_utf8()..];
            let label = rest
                .split(|c: char| glyph(c) || c == '→')
                .next()
                .unwrap_or_default()
                .trim();
            let style = styles[index]
                .get(line[..at].chars().count())
                .copied()
                .unwrap_or_default();
            DialogTab {
                label: label.to_owned(),
                answered: matches!(mark, '☒' | '☑'),
                active: style.is_filled(),
            }
        })
        .collect()
}

pub(super) fn checkbox_label(label: &str) -> Option<&str> {
    label
        .strip_prefix("[ ] ")
        .or_else(|| label.strip_prefix("[✔] "))
}

pub(super) fn text_question(above: &[&str], hint: &str, value: &str) -> Option<Dialog> {
    let question = title(above)
        .lines()
        .filter(|line| !line.starts_with(['☐', '←']))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned();
    let value = checkbox_label(value).unwrap_or(value);
    let placeholder = if value == "Type something" {
        value
    } else {
        "Type something."
    };
    // Esc would decline the whole question, every tab of a form included.
    question_dialog(question, hint, value, placeholder, None)
}

fn collect_rows(lines: &[&str], start: usize, footer: usize) -> Option<Vec<OptionLine>> {
    let mut rows: Vec<OptionLine> = Vec::new();
    let mut numbered = 0;
    let mut button = false;
    for (index, line) in lines.iter().enumerate().take(footer).skip(start) {
        if let Some(mut row) = option_line(line) {
            numbered += 1;
            if row.number != numbered {
                return None;
            }
            row.line = index;
            rows.push(row);
            continue;
        }
        let text = unboxed(line);
        let marked = marked_label(line);
        let label = marked.map_or(text.trim(), |(_, label)| label);
        if matches!(label, "Submit" | "Next") {
            if button || rows.is_empty() || rows.last()?.label == "Chat about this" {
                return None;
            }
            button = true;
            rows.push(OptionLine {
                selected: marked.is_some(),
                number: 0,
                column: indentation(text),
                label: label.to_owned(),
                scrolled_above: false,
                label_at: line[..label.as_ptr() as usize - line.as_ptr() as usize]
                    .chars()
                    .count(),
                line: index,
            });
        } else if text.trim().is_empty() || is_separator(text) {
            continue;
        } else {
            let last = rows.last_mut()?;
            if last.label == "Chat about this" || indentation(text) <= last.column {
                return None;
            }
            last.label.push(' ');
            last.label.push_str(text.trim());
        }
    }
    if rows.len() < 2 || rows.last()?.label != "Chat about this" {
        return None;
    }
    Some(rows)
}
