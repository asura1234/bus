use pulldown_cmark::{Event, Parser, Tag, TagEnd};

pub(crate) const TAG: &str = "[needs_developer_attention]";
pub(crate) const WAITING_TAGS: [&str; 2] = ["[waiting-on-you]", "[waiting_on_you]"];

pub(crate) fn text(message: &str) -> Option<&str> {
    message.trim_start().strip_prefix(TAG).map(str::trim_start)
}

/// Tags explicitly address the developer even inside agent-to-agent traffic.
pub(crate) fn waiting_tag_action(text: &str) -> Option<&str> {
    let end = waiting_tag_end(&text.to_ascii_lowercase())?;
    action_after(text, end)
}

/// Legacy whole phrases are meaningful only when the caller knows "you" is the developer.
pub(crate) fn waiting_action(text: &str) -> Option<&str> {
    const PHRASE: &str = "waiting on you";
    // ASCII case folding preserves byte offsets into the original Unicode text.
    let folded = text.to_ascii_lowercase();
    let phrase_end = folded.match_indices(PHRASE).find_map(|(start, _)| {
        let end = start + PHRASE.len();
        (!text[..start].chars().next_back().is_some_and(is_word)
            && !text[end..].chars().next().is_some_and(is_word))
        .then_some(end)
    });
    let end = [waiting_tag_end(&folded), phrase_end]
        .into_iter()
        .flatten()
        .min()?;
    action_after(text, end)
}

fn waiting_tag_end(folded: &str) -> Option<usize> {
    let code = code_ranges(folded);
    let mut offset = 0;
    for line in folded.split_inclusive(is_line_break) {
        let prose = markdown_prefix(line);
        if let Some(tag) = WAITING_TAGS.iter().find(|tag| prose.starts_with(**tag)) {
            let start = offset + line.len() - prose.len();
            if !code.iter().any(|range| range.contains(&start)) {
                return Some(start + tag.len());
            }
        }
        offset += line.len();
    }
    None
}

/// Only list and emphasis markers may precede a paging tag; backticks and prose cannot.
fn markdown_prefix(mut line: &str) -> &str {
    line = line.trim_start();
    loop {
        let before = line;
        if line.starts_with(['-', '+', '*']) && line[1..].starts_with(char::is_whitespace) {
            line = line[1..].trim_start();
        } else {
            let numbered = line.trim_start_matches(|ch: char| ch.is_ascii_digit());
            if numbered.len() < line.len()
                && numbered.starts_with(['.', ')'])
                && numbered[1..].starts_with(char::is_whitespace)
            {
                line = numbered[1..].trim_start();
            } else {
                line = line.trim_start_matches(['*', '_']).trim_start();
            }
        }
        if line == before {
            return line;
        }
    }
}

/// Use Markdown's actual code boundaries: unmatched backticks are literal text,
/// and list indentation alone does not imply an indented code block.
fn code_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut block_start = None;
    for (event, range) in Parser::new(text).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_)) => block_start = Some(range.start),
            Event::End(TagEnd::CodeBlock) => {
                if let Some(start) = block_start.take() {
                    ranges.push(start..range.end);
                }
            }
            Event::Code(_) => ranges.push(range),
            _ => {}
        }
    }
    ranges
}

fn action_after(text: &str, end: usize) -> Option<&str> {
    let action = text[end..]
        .trim_start_matches(':')
        .trim_start_matches(['*', '_', '`']);
    let mut action = action.trim_start_matches(|ch: char| ch.is_whitespace() || ch == ':');
    while !action.is_empty() {
        let line_end = action.find(is_line_break).unwrap_or(action.len());
        if action[..line_end].chars().any(char::is_alphanumeric) {
            return Some(action.trim_end());
        }
        action = action[line_end..].trim_start_matches(char::is_whitespace);
    }
    None
}

fn is_line_break(ch: char) -> bool {
    matches!(ch, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// A reserved mention is a token, not part of an email address or a longer agent name.
pub(crate) fn has_you_mention(text: &str) -> bool {
    text.to_ascii_lowercase()
        .match_indices("@you")
        .any(|(start, _)| {
            let end = start + "@you".len();
            !text[..start]
                .chars()
                .next_back()
                .is_some_and(|ch| is_word(ch) || ch == '@')
                && !text[end..].chars().next().is_some_and(is_word)
        })
}

/// Preserve the full action line, filtering terminal controls without including
/// another line or any additional message context.
pub(crate) fn plain_line(text: &str) -> String {
    text.split(['\n', '\r', '\u{2028}', '\u{2029}'])
        .next()
        .unwrap_or("")
        .chars()
        .filter_map(|ch| {
            if ch == '\t' {
                Some(' ')
            } else {
                (!ch.is_control()).then_some(ch)
            }
        })
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Keep the complete action line and bounded context (including decision options).
/// Ordinary notices still use `plain_line`; only waiting actions become multiline.
pub(crate) fn plain_action(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n");
    let mut lines = normalized.split(is_line_break).map(plain_line);
    let mut body = lines.next().unwrap_or_default();
    let mut remaining = 4096_usize.saturating_sub(body.chars().count());
    for (index, line) in lines.enumerate() {
        if index == 11 || line.chars().count() + 1 > remaining {
            body.push_str("\n…");
            break;
        }
        remaining -= line.chars().count() + 1;
        body.push('\n');
        body.push_str(&line);
    }
    body.trim_end().to_owned()
}

#[cfg(test)]
#[path = "tests/attention_test.rs"]
mod tests;
