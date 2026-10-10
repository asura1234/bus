//! Read only the live composer, excluding suggestions and transcript prompts.
use super::styled_chars;
use crate::agents::AgentKind;
use crate::terminal::runtime::InputObservation;

pub(super) fn observer(
    kind: AgentKind,
    expected: String,
) -> impl Fn(&str) -> InputObservation + Send {
    let owned = std::sync::Mutex::new(None::<String>);
    move |screen| {
        let observation = observe(kind, screen, &expected);
        let Ok(mut owned) = owned.lock() else {
            return InputObservation::Unavailable;
        };
        match observation {
            InputObservation::Empty => {
                *owned = None;
                InputObservation::Empty
            }
            InputObservation::Pending => {
                let body = input(kind, screen);
                if owned.is_some() && *owned != body {
                    return InputObservation::Other;
                }
                *owned = body;
                InputObservation::Pending
            }
            other => other,
        }
    }
}

pub(super) fn observe(kind: AgentKind, screen: &str, expected: &str) -> InputObservation {
    let Some(body) = input(kind, screen) else {
        let plain = screen
            .lines()
            .map(|line| {
                styled_chars(line)
                    .iter()
                    .map(|(c, _)| *c)
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        let detection = crate::agents::manifest::detect_with_osc(
            kind,
            crate::agents::manifest::DetectionInput {
                screen: &plain,
                osc_title: "",
                osc_progress: "",
            },
        );
        return if detection.visible_working || detection.visible_blocker {
            InputObservation::Accepted
        } else {
            InputObservation::Unavailable
        };
    };
    if body.is_empty() {
        return InputObservation::Empty;
    }
    let normalize = |text: &str| {
        text.chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
    };
    if normalize(&body) == normalize(expected)
        || collapsed_paste(kind, &body, expected)
        || kind == AgentKind::Claude && lifted_images(&body, expected, normalize)
    {
        InputObservation::Pending
    } else {
        InputObservation::Other
    }
}

/// Claude Code shows a pasted line that is one quoted image path as a leading
/// `[Image #N]` chip. The draft is ours only when every chip stands for one of
/// our image lines and the rest is exactly our remaining text.
fn lifted_images(body: &str, expected: &str, normalize: impl Fn(&str) -> String) -> bool {
    use crate::agents::providers::claude_code::hooks::{
        claude_image_placeholders, claude_lifted_images,
    };
    let (length, images) = claude_image_placeholders(body);
    let (lifted, remaining) =
        claude_lifted_images(&expected.replace("\r\n", "\n").replace('\r', "\n"));
    images > 0 && images == lifted && normalize(&body[length..]) == normalize(&remaining)
}

fn collapsed_paste(kind: AgentKind, body: &str, expected: &str) -> bool {
    // Large bracketed pastes are represented as one atomic composer element.
    if let Some(count) = body
        .strip_prefix("[Pasted Content ")
        .and_then(|text| text.strip_suffix(" chars]"))
    {
        return kind == AgentKind::Codex
            && count.parse::<usize>().ok() == Some(expected.chars().count());
    }
    let Some(label) = body
        .strip_prefix("[Pasted text #")
        .and_then(|text| text.strip_suffix(']'))
    else {
        return false;
    };
    let (id, count) = label.split_once(" +").unwrap_or((label, "0 lines"));
    if id.parse::<usize>().ok().is_none_or(|id| id == 0) {
        return false;
    }
    // Installed provider renderers differ: Claude counts newline separators;
    // Cursor counts logical lines, including the first line.
    let newlines = expected
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .matches('\n')
        .count();
    let expected_lines = match kind {
        AgentKind::Claude => newlines,
        AgentKind::Cursor => newlines + 1,
        _ => return false,
    };
    count
        .strip_suffix(" lines")
        .and_then(|count| count.parse::<usize>().ok())
        == Some(expected_lines)
}

fn input(kind: AgentKind, screen: &str) -> Option<String> {
    let lines: Vec<String> = screen
        .lines()
        .map(|line| {
            let chars = styled_chars(line);
            let plain: String = chars.iter().map(|(c, _)| *c).collect();
            if kind == AgentKind::Claude
                && !plain.contains("[Pasted text #")
                && !plain.trim().starts_with('─')
            {
                chars
                    .into_iter()
                    .filter_map(|(c, faint)| (!faint).then_some(c))
                    .collect()
            } else {
                plain
            }
        })
        .collect();
    // Cursor Agent 2026.10 draws `→` inside half-block borders; older builds `>`.
    let markers: &[char] = match kind {
        AgentKind::Claude => &['❯'],
        AgentKind::Cursor => &['>', '→'],
        _ => &['›'],
    };
    let index = lines.iter().enumerate().rev().find_map(|(index, line)| {
        let prompt = line.trim_start().trim_start_matches('│').trim_start();
        (prompt.starts_with(markers)
            && (kind != AgentKind::Claude
                || index > 0 && plain_rule(screen.lines().nth(index - 1)?)))
        .then_some(index)
    })?;
    let first = lines[index]
        .trim_start()
        .trim_start_matches('│')
        .trim_start()
        .strip_prefix(markers)?
        .trim_end_matches('│')
        .trim();
    let first = cursor_input_line(kind, first);
    let mut body = first.to_owned();
    for line in &lines[index + 1..] {
        let line = cursor_input_line(kind, line.trim().trim_end_matches('│').trim());
        // Only Codex prints its model under the composer; elsewhere a wrapped prompt
        // line may itself start with a model name.
        if line.starts_with(['─', '╰', '└', '▀'])
            || kind == AgentKind::Codex && (line.starts_with("GPT-") || line.starts_with("gpt-"))
            || line.starts_with("? for")
            || line.starts_with("/ commands")
        {
            break;
        }
        body.push('\n');
        body.push_str(line);
    }
    let mut body = body.trim().to_owned();
    if matches!(
        body.as_str(),
        "Ask Codex to do anything" | "Use /skills to list available skills" | "Ask anything"
    ) {
        body.clear();
    }
    if kind == AgentKind::Cursor
        && matches!(
            body.as_str(),
            "Plan, search, build anything"
                | "Add a follow-up"
                | "Add a follow-up — /plan to review and build"
        )
    {
        body.clear();
    }
    Some(body)
}

fn cursor_input_line(kind: AgentKind, line: &str) -> &str {
    if kind == AgentKind::Cursor {
        if let Some(draft) = line
            .strip_suffix("ctrl+c to stop")
            .filter(|draft| draft.ends_with("  "))
        {
            return draft.trim_end();
        }
    }
    line
}

fn plain_rule(line: &str) -> bool {
    styled_chars(line)
        .iter()
        .map(|(c, _)| *c)
        .collect::<String>()
        .trim()
        .starts_with('─')
}

#[cfg(test)]
#[path = "../tests/composer_test.rs"]
mod tests;
