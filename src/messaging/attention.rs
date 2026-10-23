pub(crate) const TAG: &str = "[needs_developer_attention]";
pub(crate) const WAITING_TAGS: [&str; 2] = ["[waiting-on-you]", "[waiting_on_you]"];

pub(crate) fn text(message: &str) -> Option<&str> {
    message.trim_start().strip_prefix(TAG).map(str::trim_start)
}

/// ASCII case folding preserves byte offsets into the original Unicode text.
/// The phrase also covers "still waiting on you", but not partial words.
pub(crate) fn waiting_action(text: &str) -> Option<&str> {
    const PHRASE: &str = "waiting on you";
    let folded = text.to_ascii_lowercase();
    let tag_end = crate::messaging::attention::WAITING_TAGS
        .iter()
        .filter_map(|tag| folded.find(tag).map(|start| start + tag.len()))
        .min();
    let phrase_end = folded.match_indices(PHRASE).find_map(|(start, _)| {
        let end = start + PHRASE.len();
        let is_word = |ch: char| ch.is_alphanumeric() || ch == '_';
        (!text[..start].chars().next_back().is_some_and(is_word)
            && !text[end..].chars().next().is_some_and(is_word))
        .then_some(end)
    });
    let end = [tag_end, phrase_end].into_iter().flatten().min()?;
    // Close Markdown wrapped around the marker, preserving action text after a space.
    let action = text[end..]
        .trim_start_matches(':')
        .trim_start_matches(['*', '_', '`']);
    Some(action.trim_start_matches(|ch: char| ch.is_whitespace() || ch == ':'))
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
