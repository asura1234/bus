//! Read only the live composer, excluding suggestions and transcript prompts.
use super::{styled_chars, StyledChar};
use crate::agents::AgentKind;
use crate::terminal::runtime::InputObservation;

#[cfg(test)]
mod grok_observer_tests {
    use super::*;
    #[test]
    fn grok_observer_preserves_unknown_and_edited_drafts_across_owned_observations() {
        let empty = include_str!("../../../../../tests/fixtures/grok-composer/empty.txt");
        let frame = |text: &str| empty.replace("\x1b[2mType a message...\x1b[0m", text);
        let observe = observer(AgentKind::Grok, "owned prompt".into());
        assert_eq!(observe(&frame("owned prompt")), InputObservation::Pending);
        assert_eq!(
            observe(&frame("developer edit")),
            InputObservation::Unavailable
        );
        assert_eq!(
            observe(&frame("[Pasted 12 chars]")),
            InputObservation::Unavailable
        );
        assert_eq!(observe(&frame("owned prompt")), InputObservation::Pending);
        assert_eq!(observe(empty), InputObservation::Empty);
        assert_eq!(
            observe(&frame("unrelated draft")),
            InputObservation::Unavailable
        );
    }
}

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
                if body.is_none() || owned.is_some() && *owned != body {
                    return if kind == AgentKind::Grok {
                        InputObservation::Unavailable
                    } else {
                        InputObservation::Other
                    };
                }
                *owned = body;
                InputObservation::Pending
            }
            other => other,
        }
    }
}

pub(super) fn observe(kind: AgentKind, screen: &str, expected: &str) -> InputObservation {
    if kind == AgentKind::Grok {
        return super::grok::observe(screen, expected);
    }
    let plain = screen
        .lines()
        .map(|line| {
            styled_chars(line)
                .iter()
                .map(|c| c.character)
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
    // Choosers use the same prompt marker as the composer. Inspect live
    // controls before reading input, including collapsed async questions.
    if !composer_frame(kind, &plain)
        && (detection.visible_blocker || crate::agents::dialog::parse(screen).is_some())
        || kind == AgentKind::Codex && crate::agents::dialog::codex_question_pending(screen)
    {
        return InputObservation::Unavailable;
    }
    let Some(body) = input(kind, screen) else {
        return if detection.visible_working {
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
        || kind == AgentKind::Codex && codex_image_attachment(&body, expected)
    {
        InputObservation::Pending
    } else {
        InputObservation::Other
    }
}

/// Provider chrome distinguishes editable numbered instructions from chooser
/// rows, and an empty live composer from quoted blocker text in the transcript.
fn composer_frame(kind: AgentKind, screen: &str) -> bool {
    let lines: Vec<_> = screen.lines().map(str::trim).collect();
    let marker = match kind {
        AgentKind::Claude => '❯',
        AgentKind::Codex => '›',
        _ => return false,
    };
    let Some(index) = lines.iter().rposition(|line| line.starts_with(marker)) else {
        return false;
    };
    match kind {
        AgentKind::Claude => {
            index > 0
                && lines[index - 1].starts_with('─')
                && lines[index + 1..].iter().any(|line| line.starts_with('─'))
        }
        AgentKind::Codex => lines[index + 1..]
            .iter()
            .take_while(|line| !line.starts_with(['─', '╰', '└', '▀']))
            .any(|line| {
                line.starts_with("GPT-") || line.starts_with("gpt-") || *line == "? for shortcuts"
            }),
        _ => false,
    }
}

/// Claude Code shows a pasted line that is one quoted image path as a leading
/// `[Image #N]` chip. The remaining text follows as typed, or as one collapsed
/// `[Pasted text #N +K lines]` chip when it is long (live 2026-10-21: a
/// developer message with an image was withdrawn as never shown, forever).
/// The draft is ours only when every chip stands for one of our image lines
/// and the rest is exactly our remaining text.
fn lifted_images(body: &str, expected: &str, normalize: impl Fn(&str) -> String) -> bool {
    use crate::agents::providers::claude_code::hooks::claude_lifted_images;
    let (images, rest) = image_chips(body);
    let (lifted, remaining) =
        claude_lifted_images(&expected.replace("\r\n", "\n").replace('\r', "\n"));
    images > 0
        && images == lifted
        && (normalize(rest) == normalize(&remaining)
            || collapsed_paste(AgentKind::Claude, rest, &remaining))
}

/// Codex (0.163.0) attaches a paste that is only one quoted image path and
/// shows one `[Image #N]` chip; anything with text or a second path stays text.
fn codex_image_attachment(body: &str, expected: &str) -> bool {
    use crate::agents::providers::claude_code::hooks::{
        claude_image_placeholders, claude_lifted_images,
    };
    !expected.contains(['\n', '\r'])
        && claude_image_placeholders(body) == (body.len(), 1)
        && claude_lifted_images(expected) == (1, String::new())
}

/// The leading `[Image #N]` chips of a Claude composer and the text after them.
/// The composer separates chips with one space (2.1.306), unlike the
/// submit hook's prompt.
fn image_chips(body: &str) -> (usize, &str) {
    use crate::agents::providers::claude_code::hooks::claude_image_placeholders;
    let (mut images, mut rest) = (0, body);
    loop {
        let (length, count) = claude_image_placeholders(rest);
        images += count;
        rest = &rest[length..];
        match rest.strip_prefix(' ') {
            Some(next) if count > 0 && claude_image_placeholders(next).1 > 0 => rest = next,
            _ => return (images, rest),
        }
    }
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
    if kind == AgentKind::Grok {
        return super::grok::input(screen);
    }
    let styled: Vec<_> = screen.lines().map(styled_chars).collect();
    let lines: Vec<String> = styled
        .iter()
        .map(|chars| {
            let plain: String = chars.iter().map(|c| c.character).collect();
            if kind == AgentKind::Claude
                && !plain.contains("[Pasted text #")
                && !plain.trim().starts_with('─')
            {
                chars
                    .iter()
                    .filter_map(|c| (!c.faint).then_some(c.character))
                    .collect()
            } else {
                plain
            }
        })
        .collect();
    // Cursor 2026.10.14 has a shaded box; older builds have half-block borders.
    let markers: &[char] = match kind {
        AgentKind::Claude => &['❯'],
        AgentKind::Cursor => &['>', '→'],
        _ => &['›'],
    };
    let marked_row = lines.iter().enumerate().rev().find_map(|(index, line)| {
        let prompt = line.trim_start().trim_start_matches('│').trim_start();
        (prompt.starts_with(markers)
            && (kind != AgentKind::Claude
                || index > 0 && plain_rule(screen.lines().nth(index - 1)?)))
        .then_some(index)
    });
    let background_box = (kind == AgentKind::Cursor)
        .then(|| cursor_background_composer(&styled, &lines))
        .flatten()
        // A later legacy prompt wins over older shaded history. A marker
        // inside this box is a continuation, so keep the box's first row.
        .filter(|(_, end)| marked_row.is_none_or(|row| row < *end));
    let index = background_box.map(|(start, _)| start).or(marked_row)?;
    let first = lines[index]
        .trim_start()
        .trim_start_matches('│')
        .trim_start()
        .strip_prefix(markers)?
        .trim_end_matches('│')
        .trim();
    let first = cursor_input_line(kind, first);
    let mut body = first.to_owned();
    let end = background_box.map_or(lines.len(), |(_, end)| end);
    for line in &lines[index + 1..end] {
        let line = cursor_input_line(kind, line.trim().trim_end_matches('│').trim());
        // Only Codex prints its model under the composer; elsewhere a wrapped prompt
        // line may itself start with a model name.
        if background_box.is_none()
            && (line.starts_with(['─', '╰', '└', '▀'])
                || kind == AgentKind::Codex
                    && (line.starts_with("GPT-") || line.starts_with("gpt-"))
                || line.starts_with("? for")
                || line.starts_with("/ commands"))
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

/// The first marked row in the last shaded composer starts its draft. Later
/// markers and border-like characters are input, not controls. Half-block
/// frames retain the legacy parser even when their interior is shaded.
fn cursor_background_composer(
    styled: &[Vec<StyledChar>],
    lines: &[String],
) -> Option<(usize, usize)> {
    let shaded = |row: usize| styled[row].iter().any(|c| c.background);
    let mut before = styled.len();
    while let Some(last) = (0..before).rfind(|&row| shaded(row)) {
        let start = (0..last)
            .rfind(|&row| !shaded(row))
            .map_or(0, |row| row + 1);
        let end = last + 1;
        if let Some(prompt) = (start..end).find(|&row| {
            lines[row].trim_start().starts_with(['>', '→'])
                && styled[row]
                    .iter()
                    .any(|c| c.background && matches!(c.character, '>' | '→'))
        }) {
            if start > 0 && lines[start - 1].trim_start().starts_with('▄')
                || lines
                    .get(end)
                    .is_some_and(|line| line.trim_start().starts_with('▀'))
            {
                return None;
            }
            return Some((prompt, end));
        }
        before = start;
    }
    None
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
        .map(|c| c.character)
        .collect::<String>()
        .trim()
        .starts_with('─')
}

#[cfg(test)]
#[path = "../tests/composer_test.rs"]
mod tests;
