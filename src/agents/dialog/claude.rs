//! Claude AskUserQuestion panels have unnumbered buttons and horizontal tabs.
use super::{
    highlighted, indentation, is_separator, marked_label, option_line, title, unboxed, Dialog,
    DialogKind, DialogOption, OptionLine, QuestionInput, Style,
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
        .rposition(|line| !unboxed(line).trim().is_empty())?;
    let hint = unboxed(lines[footer]).trim();
    if !hint.contains("Esc to cancel")
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
    let input = rows.iter().find(|row| row.selected).and_then(|row| {
        hint.contains("ctrl+g to edit").then(|| QuestionInput {
            value: if matches!(row.label.as_str(), "Type something." | "[ ] Type something") {
                String::new()
            } else {
                row.label.clone()
            },
            skip_key: "esc",
        })
    });
    let mut options: Vec<_> = rows
        .into_iter()
        .enumerate()
        .map(|(index, row)| DialogOption {
            number: index as u32 + 1,
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
            });
        }
    }
    Some(Dialog {
        kind: if input.is_some() {
            DialogKind::Question
        } else {
            DialogKind::Choice
        },
        input,
        text: title(&lines[header..start]),
        options,
        hint: Some(hint.to_owned()),
        fingerprint_context: format!("{:?}", styles[header]),
    })
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
                label_at: line.chars().count() - label.chars().count(),
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
