pub(crate) const TAG: &str = "[needs_developer_attention]";
pub(crate) const WAITING_TAGS: [&str; 2] = ["[waiting-on-you]", "[waiting_on_you]"];

pub(crate) fn text(message: &str) -> Option<&str> {
    message.trim_start().strip_prefix(TAG).map(str::trim_start)
}

/// Tags explicitly address the developer even inside agent-to-agent traffic.
pub(crate) fn waiting_tag_action(text: &str) -> Option<&str> {
    let end = waiting_tag_end(&text.to_ascii_lowercase())?;
    Some(action_after(text, end))
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
    Some(action_after(text, end))
}

fn waiting_tag_end(folded: &str) -> Option<usize> {
    WAITING_TAGS
        .iter()
        .filter_map(|tag| folded.find(tag).map(|start| start + tag.len()))
        .min()
}

fn action_after(text: &str, end: usize) -> &str {
    let action = text[end..]
        .trim_start_matches(':')
        .trim_start_matches(['*', '_', '`']);
    action.trim_start_matches(|ch: char| ch.is_whitespace() || ch == ':')
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
