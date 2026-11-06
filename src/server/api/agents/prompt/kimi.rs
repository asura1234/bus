//! Conservative observation of Kimi Code's current framed composer.
use super::styled_chars;
use crate::terminal::runtime::InputObservation;

pub(super) fn working(screen: &str) -> bool {
    let plain = super::plain_screen(screen);
    let detected = crate::agents::manifest::detect_with_osc(
        crate::agents::AgentKind::Kimi,
        crate::agents::manifest::DetectionInput {
            screen: &plain,
            osc_title: "",
            osc_progress: "",
        },
    );
    detected.visible_working && !detected.visible_blocker
}

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

fn frame(screen: &str) -> Option<(Vec<String>, usize, usize)> {
    let lines: Vec<String> = screen
        .lines()
        .map(|line| styled_chars(line).iter().map(|c| c.character).collect())
        .collect();
    let plain = lines.join("\n");
    let detection = crate::agents::manifest::detect_with_osc(
        crate::agents::AgentKind::Kimi,
        crate::agents::manifest::DetectionInput {
            screen: &plain,
            osc_title: "",
            osc_progress: "",
        },
    );
    if detection.visible_blocker || crate::terminal::driver::has_input_controls(&plain) {
        return None;
    }
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
        lowercase.contains("trust this folder?")
            || lowercase.contains("run this command?")
            || lowercase.contains("write this file?")
            || lowercase.contains("askuserquestion")
    }) {
        return None;
    }
    // The last box must actually be a composer, not the welcome card.
    lines[top + 1].trim_start().strip_prefix("│ > ")?;
    Some((lines, top, bottom))
}

fn rows(screen: &str) -> Option<(Vec<String>, bool)> {
    let (lines, top, bottom) = frame(screen)?;
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
