//! Conservative observation of Antigravity CLI (`agy`) framed composer.
use super::styled_chars;
use crate::terminal::runtime::InputObservation;

pub(super) fn observe(screen: &str, expected: &str) -> InputObservation {
    let lines: Vec<String> = screen
        .lines()
        .map(|line| styled_chars(line).iter().map(|c| c.character).collect())
        .collect();
    let plain = lines.join("\n");
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
    let Some(rows) = rows_from_lines(&lines) else {
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

pub(super) fn input(screen: &str) -> Option<String> {
    let lines: Vec<String> = screen
        .lines()
        .map(|line| styled_chars(line).iter().map(|c| c.character).collect())
        .collect();
    let plain = lines.join("\n");
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
    Some(rows_from_lines(&lines)?.join("\n"))
}

fn normalize(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

fn is_rule(line: &str) -> bool {
    let trimmed = line.trim();
    let count = trimmed.chars().count();
    count >= 10 && trimmed.chars().all(|c| c == '─')
}

fn rows_from_lines(lines: &[String]) -> Option<Vec<String>> {
    let bottom = lines.iter().rposition(|line| is_rule(line))?;
    let top = (0..bottom).rev().find(|&index| is_rule(&lines[index]))?;
    if bottom <= top + 1 || bottom - top > 64 {
        return None;
    }
    let footer = lines[bottom + 1..]
        .iter()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_ascii_lowercase())?;
    if !footer.starts_with("? for shortcuts") && !footer.starts_with("esc to cancel") {
        return None;
    }
    let interior = &lines[top + 1..bottom];
    let prompt_rel = interior.iter().position(|line| !line.trim().is_empty())?;
    let prompt_line = interior[prompt_rel].trim_start();
    let first = prompt_line
        .strip_prefix("> ")
        .or_else(|| prompt_line.strip_prefix('>'))?
        .trim_end()
        .to_owned();
    let mut rows = vec![first];
    for cont in &interior[prompt_rel + 1..] {
        let stripped = cont.strip_prefix("  ").unwrap_or(cont.as_str()).trim_end();
        rows.push(stripped.to_owned());
    }
    while rows.len() > 1 && rows.last().is_some_and(String::is_empty) {
        rows.pop();
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
