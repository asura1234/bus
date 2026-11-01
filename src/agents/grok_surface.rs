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
    let edge = [
        ('╭', '╮', false),
        ('┌', '┐', false),
        ('╰', '╯', true),
        ('└', '┘', true),
    ]
    .into_iter()
    .find_map(|(left, right, bottom)| {
        line.strip_prefix(left)
            .and_then(|s| s.strip_suffix(right))
            .map(|inner| (inner, bottom))
    });
    let Some((inner, bottom)) = edge else {
        return false;
    };
    // The model, mode and remaining allowance are changing labels inside the
    // lower edge. Callers separately require the framed ❯ prompt row.
    inner.chars().take_while(|c| *c == '─').count() >= 3
        && inner.ends_with('─')
        && !inner.chars().any(char::is_control)
        && (bottom || inner.chars().all(|c| c == '─'))
}
