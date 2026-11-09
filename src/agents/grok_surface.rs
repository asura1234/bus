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
    plain_box(line) || labeled_grok_border(line)
}

fn plain_box(line: &str) -> bool {
    line.chars().filter(|c| *c == '─').count() >= 3
        && line
            .chars()
            .all(|c| matches!(c, '─' | '╭' | '╮' | '╰' | '╯' | '┌' | '┐' | '└' | '┘'))
}

/// A labelled bottom rule is a composer only for `Grok {model} ─` or the
/// captured quota label `Weekly limit left: N% · Grok {model} ─`.
/// Other text between the corners is not a composer.
fn labeled_grok_border(line: &str) -> bool {
    let Some(inner) = line.strip_prefix('╰').and_then(|s| s.strip_suffix('╯')) else {
        return false;
    };
    let Some((rule, label)) = inner.split_once(' ') else {
        return false;
    };
    let Some(label) = label.strip_suffix(" ─") else {
        return false;
    };
    rule_run(rule) && grok_label(label)
}

fn grok_label(label: &str) -> bool {
    if let Some(model) = label.strip_prefix("Grok ") {
        return model_label(Some(model));
    }
    let Some((prefix, tail)) = label.rsplit_once(" · Grok ") else {
        return false;
    };
    weekly_limit_prefix(prefix) && weekly_tail(tail)
}

/// The quota label's tail after `Grok`: one model token, an optional
/// `(effort)` token, and an optional `· status` token. `4.7 (high) ·
/// always-approve` passes; prose with spaces in the model slot does not.
fn weekly_tail(text: &str) -> bool {
    let Some(rest) = take_border_token(text) else {
        return false;
    };
    let rest = match rest.strip_prefix(" (") {
        Some(inner) => {
            let Some(rest) = take_border_token(inner) else {
                return false;
            };
            let Some(rest) = rest.strip_prefix(')') else {
                return false;
            };
            rest
        }
        None => rest,
    };
    let rest = rest.strip_prefix(" · ").unwrap_or(rest);
    rest.is_empty() || take_border_token(rest).is_some_and(str::is_empty)
}

/// One border-label token: ASCII alphanumeric plus `.` and `-`, never a space.
fn take_border_token(text: &str) -> Option<&str> {
    let end = text
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-')))
        .unwrap_or(text.len());
    (end > 0).then(|| &text[end..])
}

fn weekly_limit_prefix(prefix: &str) -> bool {
    let Some(percent) = prefix
        .strip_prefix("Weekly limit left: ")
        .and_then(|rest| rest.strip_suffix('%'))
    else {
        return false;
    };
    percent.bytes().all(|ch| ch.is_ascii_digit())
        && percent.parse::<u8>().is_ok_and(|value| value <= 100)
}

fn rule_run(text: &str) -> bool {
    text.chars().count() >= 3 && text.chars().all(|c| c == '─')
}

fn model_label(model: Option<&str>) -> bool {
    let Some(model) = model else {
        return false;
    };
    !model.is_empty()
        && model.len() <= 80
        && model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '(' | ')' | ' ' | '-' | '·'))
}

/// Idle composer the synthetic Grok provider draws for the Grok 4.7 quota
/// border captured on 2026-11-09. The `❯` row is empty. The bottom label
/// starts with the weekly-limit prefix, and the footer is the live
/// `Shift+Tab` / `Ctrl+.` hint.
#[cfg(any(test, feature = "dev-tools"))]
pub(crate) fn weekly_limit_idle_screen() -> String {
    "  ╭────────────────────────────────────────────────────────────────────────╮\n  │ ❯                                                                      │\n  ╰───────────── Weekly limit left: 6% · Grok 4.7 (high) · always-approve ─╯\n\n  Shift+Tab:mode  │  Ctrl+.:shortcuts\n".to_owned()
}

#[cfg(test)]
mod tests {
    #[test]
    fn multi_recipient_grok_restored_header_clears_the_entire_prompt() {
        let screen = "╭────────────────────╮\n│ ❯ [Bus message 42 from you to app master 2, builder [deleted]] │\n│   shared task │\n╰────────────────────╯";
        assert_eq!(super::restored_prompt_clear_keys(screen).unwrap().len(), 6);
        assert!(super::restored_prompt_clear_keys(&format!("[stop]\n{screen}")).is_none());
        assert!(super::restored_prompt_clear_keys(
            &screen.replace("[Bus message", "developer note")
        )
        .is_none());
    }
}
