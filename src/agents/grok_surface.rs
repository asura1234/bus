//! Version-bound Grok composer framing shared by input and dialog observers.
/// One owned line-kill per visible composer row when Ctrl+C has put Bus's
/// prompt back. The count is the whole framed body. Capping it deletes the
/// envelope and leaves the continuation, which no longer starts with
/// `[Bus message `, on an idle composer. `[stop]` means the turn is still
/// cancelling. A developer draft does not start with the Bus envelope.
pub(crate) fn restored_prompt_clear_keys(screen: &str) -> Option<Vec<String>> {
    if screen.contains("[stop]") {
        return None;
    }
    let lines: Vec<&str> = screen.lines().collect();
    let bottom = lines.iter().rposition(|line| composer_border(line))?;
    let top = lines[..bottom]
        .iter()
        .rposition(|line| composer_border(line))?;
    if top + 1 >= bottom {
        return None;
    }
    let first = framed_content(lines[top + 1])?;
    let body = first.trim_start().strip_prefix('❯')?.trim_start();
    if !body.starts_with("[Bus message ") {
        return None;
    }
    let mut count = 1usize;
    for line in &lines[top + 2..bottom] {
        framed_content(line)?;
        count += 1;
    }
    let mut keys = Vec::with_capacity(count * 3);
    for _ in 0..count {
        keys.extend(["ctrl+a", "ctrl+k", "ctrl+u"].map(str::to_owned));
    }
    Some(keys)
}

fn framed_content(line: &str) -> Option<&str> {
    let line = line.trim_end();
    match line.trim_start().strip_prefix('│') {
        Some(inner) => Some(inner.strip_suffix('│')?.trim_end()),
        None => Some(line),
    }
}

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
