//! Conservative observation of Antigravity CLI (`agy`) framed composer.
use super::styled_chars;
use crate::terminal::runtime::InputObservation;

pub(super) fn observe(
    screen: &str,
    expected: &str,
    cursor: Option<(u16, u16)>,
) -> InputObservation {
    let styled: Vec<_> = screen.lines().map(styled_chars).collect();
    let plain_lines: Vec<String> = styled
        .iter()
        .map(|chars| chars.iter().map(|c| c.character).collect())
        .collect();
    let plain = plain_lines.join("\n");
    let detection = crate::agents::manifest::detect_with_osc(
        crate::agents::AgentKind::Antigravity,
        crate::agents::manifest::DetectionInput {
            screen: &plain,
            osc_title: "",
            osc_progress: "",
        },
    );
    if detection.visible_blocker || crate::terminal::driver::has_input_controls(&plain) {
        return InputObservation::Unavailable;
    }
    let Some(rows) = rows_from_styled(&styled, &plain_lines, cursor) else {
        return if detection.visible_working {
            InputObservation::Accepted
        } else {
            InputObservation::Unavailable
        };
    };
    if rows.len() == 1 && rows[0].is_empty() {
        return InputObservation::Empty;
    }
    let body = rows.join("\n");
    let trimmed = body.trim();
    let expected = expected.replace("\r\n", "\n").replace('\r', "\n");
    if collapsed_paste(trimmed, &expected) {
        return InputObservation::Pending;
    }
    if trimmed.contains("[Pasted text #") || trimmed.contains("[Image #") {
        return InputObservation::Unavailable;
    }
    if matches_rows(&rows, &expected) || normalize(trimmed) == normalize(&expected) {
        InputObservation::Pending
    } else {
        InputObservation::Other
    }
}

pub(super) fn input(screen: &str, cursor: Option<(u16, u16)>) -> Option<String> {
    let styled: Vec<_> = screen.lines().map(styled_chars).collect();
    let plain_lines: Vec<String> = styled
        .iter()
        .map(|chars| chars.iter().map(|c| c.character).collect())
        .collect();
    let plain = plain_lines.join("\n");
    let detection = crate::agents::manifest::detect_with_osc(
        crate::agents::AgentKind::Antigravity,
        crate::agents::manifest::DetectionInput {
            screen: &plain,
            osc_title: "",
            osc_progress: "",
        },
    );
    if detection.visible_blocker || crate::terminal::driver::has_input_controls(&plain) {
        return None;
    }
    Some(rows_from_styled(&styled, &plain_lines, cursor)?.join("\n"))
}

fn normalize(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

fn is_rule(line: &str) -> bool {
    let trimmed = line.trim();
    let count = trimmed.chars().count();
    count >= 10 && trimmed.chars().all(|c| c == '─')
}

fn is_mode_placeholder(text: &str) -> bool {
    [
        "Auto mode: edits & safe commands auto-approved",
        "Accept-edits mode: file edits auto-approved",
        "--dangerously-skip-permissions: auto-approving all tool permissions",
        "Plan mode:",
    ]
    .iter()
    .any(|prefix| {
        text.starts_with(prefix)
            || (prefix.starts_with(text.trim_end_matches("..."))
                && text.ends_with("...")
                && text.len() >= 16)
    })
}

fn rows_from_styled(
    styled: &[Vec<super::StyledChar>],
    plain_lines: &[String],
    cursor: Option<(u16, u16)>,
) -> Option<Vec<String>> {
    let bottom = plain_lines.iter().rposition(|line| is_rule(line))?;
    let top = (0..bottom).rev().find(|&index| is_rule(&plain_lines[index]))?;
    if bottom <= top + 1 || bottom - top > 64 {
        return None;
    }
    let footer = plain_lines[bottom + 1..]
        .iter()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_ascii_lowercase())?;
    if !footer.starts_with("? for shortcuts") && !footer.starts_with("esc to cancel") {
        return None;
    }
    let interior = &plain_lines[top + 1..bottom];
    let prompt_rel = interior.iter().position(|line| !line.trim().is_empty())?;
    let prompt_idx = top + 1 + prompt_rel;

    // Strip SGR faint placeholder cells unless a paste chip is present.
    let render_line = |idx: usize| -> String {
        let plain = &plain_lines[idx];
        if plain.contains("[Pasted text #") {
            plain.clone()
        } else {
            styled[idx]
                .iter()
                .filter_map(|c| (!c.faint).then_some(c.character))
                .collect()
        }
    };

    let prompt_rendered = render_line(prompt_idx);
    let prompt_trimmed = prompt_rendered.trim_start();
    let mut first = prompt_trimmed
        .strip_prefix("> ")
        .or_else(|| prompt_trimmed.strip_prefix('>'))?
        .trim_end()
        .to_owned();

    let mut rows = vec![first.clone()];
    for idx in prompt_idx + 1..bottom {
        let cont = render_line(idx);
        let stripped = cont.strip_prefix("  ").unwrap_or(cont.as_str()).trim_end();
        rows.push(stripped.to_owned());
    }
    while rows.len() > 1 && rows.last().is_some_and(String::is_empty) {
        rows.pop();
    }

    // AGY paints its execution-mode placeholder starting at column 2 while the
    // composer is empty (cursor parked at column 2 on the prompt row).
    if rows.len() == 1 {
        let cursor_at_prompt_start =
            cursor.is_some_and(|(col, row)| col <= 2 && usize::from(row) == prompt_idx);
        if is_mode_placeholder(&first) || (cursor_at_prompt_start && is_mode_placeholder(plain_lines[prompt_idx].trim_start().trim_start_matches('>').trim_start())) {
            first.clear();
            rows[0] = first;
        }
    }
    Some(rows)
}

fn collapsed_paste(body: &str, expected: &str) -> bool {
    let Some(label) = body
        .strip_prefix("[Pasted text #")
        .and_then(|rest| rest.strip_suffix(']'))
    else {
        return false;
    };
    if let Some((id, chars_part)) = label.split_once(' ') {
        if id.parse::<usize>().ok().is_none_or(|n| n == 0) {
            return false;
        }
        if let Some(count_str) = chars_part.strip_suffix(" chars") {
            return count_str.parse::<usize>().ok() == Some(expected.chars().count());
        }
        if let Some(lines_str) = chars_part
            .strip_prefix('+')
            .and_then(|rest| rest.strip_suffix(" lines"))
        {
            let Ok(lines_count) = lines_str.parse::<usize>() else {
                return false;
            };
            let newlines = expected.matches('\n').count();
            return lines_count == newlines || lines_count == newlines + 1;
        }
    }
    false
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
