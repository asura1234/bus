//! Generic input-control evidence, never provider vocabulary or answer policy.
use super::Grid;
use sha2::{Digest, Sha256};

/// A stable episode key from visible controls. Observation always exposes the
/// entire styled screen, including surfaces whose controls are ambiguous.
pub(crate) fn blocker(grid: &Grid) -> Option<String> {
    let rows = grid.rows_text();
    let end = controls_end(&rows)?;
    let mut hash = Sha256::new();
    // Include nearby question/command context, without focus-marker churn.
    // A bounded control region excludes older transcript and working chrome.
    for row in &rows[end.saturating_sub(12)..=end] {
        let row = row.trim().trim_matches(['│', '┃', '║']).trim();
        let row = row
            .trim_start_matches(['❯', '›', '>', '●', '◉', '▶', '→'])
            .trim();
        if !row.is_empty() {
            hash.update(row.as_bytes());
            hash.update(b"\0");
        }
    }
    Some(format!("{:x}", hash.finalize()))
}

/// Composer inspectors already decoded their screen; share the control test
/// without decoding ANSI or reading the terminal a second time.
pub(crate) fn has_input_controls(screen: &str) -> bool {
    controls_end(&screen.lines().map(str::to_owned).collect::<Vec<_>>()).is_some()
}

fn controls_end(rows: &[String]) -> Option<usize> {
    let meaningful = |row: &str| {
        let row = row.trim().trim_matches(['│', '┃', '║']).trim();
        !row.is_empty() && !row.chars().all(|ch| "─━═-╭╮╰╯┌┐└┘ ".contains(ch))
    };
    let end = rows.iter().rposition(|row| meaningful(row))?;
    let start = end.saturating_sub(14);
    let numbered =
        |row: &str| super::selection::option_body(row).0 != super::selection::option_label(row).0;
    let menu = rows[start..=end].iter().filter(|row| numbered(row)).count() >= 2
        && rows[start..=end]
            .iter()
            .any(|row| numbered(row) && super::selection::option_body(row).1);
    let footer = |row: &str| {
        let row = row.trim();
        !meaningful(row)
            || (!super::selection::option_body(row).1 && row.contains(" · "))
            || row.starts_with("? ")
            || row == "⟩"
    };
    for index in (start..=end).rev() {
        let lower = rows[index].trim().to_lowercase();
        let pending_answer =
            (lower.starts_with("shift+") || lower.starts_with('⇧')) && lower.contains("to answer");
        if pending_answer
            && rows[start..index].iter().rev().take(2).any(|row| {
                row.trim()
                    .strip_prefix("? ")
                    .is_some_and(|tail| tail.starts_with(|ch: char| ch.is_ascii_digit()))
            })
        {
            return Some(index);
        }
        if !rows[index + 1..=end].iter().all(|row| footer(row)) {
            continue;
        }
        if control_row(&rows[index])
            || menu
                && ((lower.contains("enter") && lower.starts_with("press "))
                    || (lower.starts_with("esc") && lower.contains("cancel")))
        {
            return Some(index);
        }
    }
    if menu
        && rows[start..=end].iter().any(|row| {
            numbered(row)
                && row
                    .rsplit_once('(')
                    .and_then(|(_, tail)| tail.strip_suffix(')'))
                    .is_some_and(|key| super::input::parse_chord(key).is_some())
        })
    {
        let last = rows[start..=end].iter().rposition(|row| numbered(row))? + start;
        return rows[last + 1..=end]
            .iter()
            .all(|row| footer(row))
            .then_some(last);
    }
    None
}

fn control_row(row: &str) -> bool {
    let row = row
        .trim()
        .trim_matches(['│', '┃', '║'])
        .trim()
        .to_lowercase();
    if ["[y/n]", "[n/y]", "(y/n)", "(n/y)", "[yes/no]"]
        .iter()
        .any(|hint| row.contains(hint))
    {
        return true;
    }
    let starts_with_key = [
        "enter",
        "esc",
        "press ",
        "use arrow",
        "↑",
        "↓",
        "←",
        "→",
        "tab",
        "shift+",
        "ctrl+",
        "⇧",
        "⌃",
    ]
    .iter()
    .any(|key| row.starts_with(key));
    starts_with_key
        && (row.contains("to answer")
            || (row.contains("enter")
                && ["select", "confirm", "submit", "accept", "continue"]
                    .iter()
                    .any(|verb| row.contains(verb))
                && ["esc", "skip", "cancel", "navigate", "↑", "↓"]
                    .iter()
                    .any(|alternate| row.contains(alternate)))
            || ((row.contains('↑') || row.contains('↓'))
                && (row.contains("select") || row.contains("navigate"))))
}
