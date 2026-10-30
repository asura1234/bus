//! Version-bound Grok composer framing shared by input and dialog observers.
pub(crate) fn composer_border(line: &str) -> bool {
    let line = line.trim();
    let rules = |text: &str| text.chars().count() >= 3 && text.chars().all(|c| c == '─');
    if line.chars().filter(|c| *c == '─').count() >= 3
        && line
            .chars()
            .all(|c| matches!(c, '─' | '╭' | '╮' | '╰' | '╯' | '┌' | '┐' | '└' | '┘'))
    {
        return true;
    }
    let Some(inner) = line.strip_prefix('╰').and_then(|s| s.strip_suffix('╯')) else {
        return false;
    };
    let Some((rule, label)) = inner.split_once(' ') else {
        return false;
    };
    let Some(model) = label
        .strip_prefix("Grok ")
        .and_then(|s| s.strip_suffix(" ─"))
    else {
        return false;
    };
    rules(rule)
        && !model.is_empty()
        && model.len() <= 80
        && model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '(' | ')' | ' ' | '-' | '·'))
}
