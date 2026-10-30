//! Conservative observation of Kimi Code's current framed composer.
use super::styled_chars;
use crate::terminal::runtime::InputObservation;

pub(super) fn observe(screen: &str, expected: &str) -> InputObservation {
    let Some(rows) = rows(screen) else {
        return InputObservation::Unavailable;
    };
    if rows.len() == 1 && rows[0].is_empty() {
        return InputObservation::Empty;
    }
    let expected = expected.replace("\r\n", "\n").replace('\r', "\n");
    if matches_rows(&rows, &expected) {
        InputObservation::Pending
    } else {
        InputObservation::Unavailable
    }
}

pub(super) fn input(screen: &str) -> Option<String> {
    Some(rows(screen)?.join("\n"))
}

fn matches_rows(rows: &[String], expected: &str) -> bool {
    let Some(first) = rows.first() else {
        return false;
    };
    let Some(rest) = expected.strip_prefix(first) else {
        return false;
    };
    let mut positions = vec![expected.len() - rest.len()];
    for row in &rows[1..] {
        let mut next = Vec::new();
        for position in positions {
            let rest = &expected[position..];
            for separator in ["\n", " ", ""] {
                if let Some(after) = rest
                    .strip_prefix(separator)
                    .and_then(|tail| tail.strip_prefix(row))
                {
                    next.push(expected.len() - after.len());
                }
            }
        }
        next.sort_unstable();
        next.dedup();
        if next.is_empty() {
            return false;
        }
        positions = next;
    }
    positions.contains(&expected.len())
}

fn rows(screen: &str) -> Option<Vec<String>> {
    let lines: Vec<String> = screen
        .lines()
        .map(|line| styled_chars(line).iter().map(|c| c.character).collect())
        .collect();
    let bottom = lines
        .iter()
        .rposition(|line| line.trim().starts_with('╰') && line.contains('╯'))?;
    let top = (0..bottom)
        .rev()
        .find(|&index| lines[index].trim().starts_with('╭') && lines[index].contains('╮'))?;
    if bottom <= top + 1 || bottom - top > 64 {
        return None;
    }
    let preceding = &lines[top.saturating_sub(16)..top];
    if preceding.iter().any(|line| {
        let line = line.trim().to_lowercase();
        line.contains("working…")
            || line.contains("working...")
            || line.contains("trust this folder?")
            || line.contains("run this command?")
            || line.contains("write this file?")
            || line.contains("askuserquestion")
    }) {
        return None;
    }
    let first = lines[top + 1].trim_start().strip_prefix("│ > ")?;
    let first = first.strip_suffix('│')?.trim_end();
    let mut rows = vec![first.to_owned()];
    for line in &lines[top + 2..bottom] {
        let continuation = line.trim_start().strip_prefix("│   ")?;
        rows.push(continuation.strip_suffix('│')?.trim_end().to_owned());
    }
    Some(rows)
}
