//! Conservative observation of Kimi Code's current framed composer.
use super::styled_chars;
use crate::terminal::runtime::InputObservation;

pub(super) fn observe(screen: &str, expected: &str) -> InputObservation {
    let Some((rows, clipped)) = rows(screen) else {
        return InputObservation::Unavailable;
    };
    // A visible suffix never establishes ownership of hidden rows, even if
    // it happens to equal the whole requested message.
    if clipped {
        return InputObservation::Unavailable;
    }
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
    Some(rows(screen)?.0.join("\n"))
}

/// `/new` can open its completion menu after the editor has closed. Only that
/// known repaint is allowed; the full draft was verified in the native editor
/// and the input lease must still be held by the caller.
pub(super) fn same_new_editor_frame(previous: &str, current: &str) -> bool {
    fn tail(screen: &str) -> Option<(Vec<String>, bool)> {
        let (draft, clipped) = rows(screen)?;
        if clipped || draft != ["/new"] {
            return None;
        }
        let lines: Vec<String> = screen
            .lines()
            .map(|line| styled_chars(line).iter().map(|c| c.character).collect())
            .collect();
        if lines.iter().any(|line| {
            let line = line.to_lowercase();
            [
                "working…",
                "working...",
                "trust this folder?",
                "run this command?",
                "write this file?",
                "askuserquestion",
            ]
            .iter()
            .any(|blocker| line.contains(blocker))
        }) {
            return None;
        }
        let bottom = lines
            .iter()
            .rposition(|line| line.trim().starts_with('╰') && line.contains('╯'))?;
        let mut result = Vec::new();
        let mut menu = false;
        let mut remaining = lines[bottom + 1..].iter().peekable();
        while let Some(line) = remaining.next() {
            let text = line.trim();
            if text.starts_with("│   → new") && text.contains("Start a fresh session in") {
                let continuation = remaining.next()?.trim();
                if !continuation.starts_with("│                 the current workspace") {
                    return None;
                }
                menu = true;
            } else if !text.is_empty() {
                result.push(text.to_owned());
            }
        }
        Some((result, menu))
    }
    match (tail(previous), tail(current)) {
        (Some((a, menu_a)), Some((b, menu_b))) => (menu_a || menu_b) && a == b,
        _ => false,
    }
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

fn rows(screen: &str) -> Option<(Vec<String>, bool)> {
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
        let line = line.trim();
        let lowercase = line.to_lowercase();
        let mut chars = line.chars();
        let first = chars.next().unwrap_or_default();
        let rest = chars.as_str().trim_start().to_lowercase();
        let active_spinner = (('\u{2800}'..='\u{28ff}').contains(&first)
            && [
                "thinking…",
                "thinking...",
                "working…",
                "working...",
                "using ",
            ]
            .iter()
            .any(|prefix| rest.starts_with(prefix)))
            || (matches!(first, '🌕' | '🌖' | '🌗' | '🌘' | '🌑' | '🌒' | '🌓' | '🌔')
                && rest.is_empty());
        active_spinner
            || lowercase.contains("trust this folder?")
            || lowercase.contains("run this command?")
            || lowercase.contains("write this file?")
            || lowercase.contains("askuserquestion")
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
    let clipped = [top, bottom].iter().any(|&index| {
        let line = &lines[index];
        (line.contains('↑') || line.contains('↓')) && line.contains("more")
    });
    Some((rows, clipped))
}
