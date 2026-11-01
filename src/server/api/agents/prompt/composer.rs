//! Read only the live composer, excluding suggestions and transcript prompts.
use super::{styled_chars, StyledChar};
use crate::agents::AgentKind;
use crate::terminal::runtime::InputObservation;

mod diagnostics;
mod ownership;
mod preservation;

#[cfg(test)]
pub(super) fn preserving_observer(
    kind: AgentKind,
    expected: String,
    directory: std::path::PathBuf,
    keys: Vec<bytes::Bytes>,
) -> impl crate::terminal::runtime::SubmissionInspector {
    preserving_observer_for_submission(kind, expected, directory, keys, false)
}

pub(super) fn preserving_observer_for_submission(
    kind: AgentKind,
    expected: String,
    directory: std::path::PathBuf,
    keys: Vec<bytes::Bytes>,
    steer: bool,
) -> impl crate::terminal::runtime::SubmissionInspector {
    let keys = if kind == AgentKind::Claude && !preservation::default_claude_stash_binding() {
        Vec::new()
    } else {
        keys
    };
    preservation::PreservingComposer::new(kind, expected, directory, keys).with_steering(steer)
}

pub(super) fn observer(
    kind: AgentKind,
    expected: String,
) -> impl crate::terminal::runtime::SubmissionInspector {
    ownership::OwnedComposer::new(kind, expected)
}

#[cfg(test)]
pub(super) fn observe(kind: AgentKind, screen: &str, expected: &str) -> InputObservation {
    observe_with_cursor(kind, screen, expected, None)
}

pub(super) fn observe_with_cursor(
    kind: AgentKind,
    screen: &str,
    expected: &str,
    cursor: Option<(u16, u16)>,
) -> InputObservation {
    observe_since_launch(kind, screen, expected, cursor, false)
}

pub(super) fn observe_since_launch(
    kind: AgentKind,
    screen: &str,
    expected: &str,
    cursor: Option<(u16, u16)>,
    pristine: bool,
) -> InputObservation {
    if kind == AgentKind::Grok {
        return super::grok::observe(screen, expected);
    }
    if kind == AgentKind::Kimi {
        return super::kimi::observe(screen, expected);
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
    let Some(body) = input_since_launch(kind, screen, cursor, pristine) else {
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

/// Codex either attaches or inserts each dedicated image paste as literal text.
/// Own the draft only when every image and the remaining text are accounted for.
fn codex_image_attachment(body: &str, expected: &str) -> bool {
    let (expected_images, remaining) =
        crate::agents::providers::codex::hooks::image_paste_parts(expected);
    let normalize = |text: &str| {
        text.chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
    };
    let expected_images: Vec<_> = expected_images
        .iter()
        .map(|image| normalize(image))
        .collect();
    let body = normalize(body);
    !expected_images.is_empty()
        && crate::agents::providers::codex::hooks::text_after_image_pastes(
            &body,
            &expected_images,
            "[Image#",
        )
        .is_some_and(|text| {
            text == normalize(&remaining)
                || text == format!("[PastedContent{}chars]", remaining.chars().count())
        })
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
    input_with_cursor(kind, screen, None)
}

fn input_with_cursor(kind: AgentKind, screen: &str, cursor: Option<(u16, u16)>) -> Option<String> {
    input_since_launch(kind, screen, cursor, false)
}

fn input_since_launch(
    kind: AgentKind,
    screen: &str,
    cursor: Option<(u16, u16)>,
    pristine: bool,
) -> Option<String> {
    if kind == AgentKind::Grok {
        return super::grok::input(screen);
    }
    if kind == AgentKind::Kimi {
        return super::kimi::input(screen);
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
    if crate::agents::is_empty_codex_composer(&body) || body == "Ask anything" {
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
    // Claude Code 2.1.309 paints a fresh-launch placeholder as the entire
    // composer line `Try "..."`, cursor on the first content cell, and Enter
    // does not submit it. Captured raw PTY bytes for that placeholder, a
    // Tab-accepted draft, and a literally typed `Try "fix typecheck errors"`
    // then Ctrl+A all contain zero SGR, so colour cannot separate them.
    // Clear the line only while this pane is still untouched since launch.
    // The same shape after a developer key or a Bus paste is a draft.
    if pristine
        && kind == AgentKind::Claude
        && claude_startup_suggestion(&body)
        && cursor_on_claude_suggestion(screen, cursor)
    {
        body.clear();
    }
    Some(body)
}

fn claude_startup_suggestion(body: &str) -> bool {
    let Some(inner) = body
        .strip_prefix("Try \"")
        .and_then(|rest| rest.strip_suffix('"'))
    else {
        return false;
    };
    !inner.is_empty() && !inner.contains('"')
}

fn cursor_on_claude_suggestion(screen: &str, cursor: Option<(u16, u16)>) -> bool {
    use unicode_width::UnicodeWidthChar;
    let Some((x, y)) = cursor else {
        return false;
    };
    let lines: Vec<String> = screen
        .lines()
        .map(|line| {
            styled_chars(line)
                .iter()
                .map(|cell| cell.character)
                .collect()
        })
        .collect();
    let Some(index) = lines.iter().enumerate().rev().find_map(|(index, line)| {
        let prompt = line.trim_start();
        (prompt.starts_with('❯') && index > 0 && lines[index - 1].trim_start().starts_with('─'))
            .then_some(index)
    }) else {
        return false;
    };
    if usize::from(y) != index {
        return false;
    }
    let line = &lines[index];
    let Some(marker) = line.find('❯') else {
        return false;
    };
    let mut column = line[..marker]
        .chars()
        .map(|c| c.width().unwrap_or(0))
        .sum::<usize>()
        + 1;
    for c in line[marker + '❯'.len_utf8()..].chars() {
        if !c.is_whitespace() {
            break;
        }
        column += c.width().unwrap_or(0);
    }
    usize::from(x) == column
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

#[cfg(test)]
#[path = "../tests/codex_image_test.rs"]
mod codex_image_tests;

#[cfg(test)]
#[path = "../tests/kimi_screen_test.rs"]
mod kimi_screen_tests;
