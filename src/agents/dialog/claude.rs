//! Claude AskUserQuestion panels have unnumbered buttons and horizontal tabs.
use super::{
    highlighted, indentation, is_separator, marked_label, option_line, question_dialog, title,
    unboxed, Dialog, DialogKind, DialogOption, OptionLine, Style,
};

/// Claude's ruled editable composer can contain the same numbered text and
/// marker as a chooser. Inspect only the last numbered block's enclosing frame,
/// so an earlier transcript composer cannot hide a later real permission dialog.
pub(super) fn composer_contains(lines: &[&str]) -> bool {
    let Some(start) = lines
        .iter()
        .rposition(|line| option_line(line).is_some_and(|option| option.number == 1))
    else {
        return false;
    };
    let Some(top) = lines[..start]
        .iter()
        .rposition(|line| is_separator(unboxed(line)))
    else {
        return false;
    };
    unboxed(lines[top + 1]).trim_start().starts_with('❯')
        && lines[start + 1..]
            .iter()
            .any(|line| is_separator(unboxed(line)))
}

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
    let mut options: Vec<_> = rows
        .into_iter()
        .enumerate()
        .map(|(index, row)| DialogOption {
            number: index as u32 + 1,
            identity_label: checkbox_label(&row.label).map(str::to_owned),
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
    })
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
    question_dialog(question, hint, value, placeholder, "esc")
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
