//! Editable numbered instructions share the marker of a selected menu option.
use super::{is_separator, numbered_options, option_line, unboxed};

/// Inspect the last numbered block's own frame: transcript composers must not
/// suppress a later real permission dialog.
pub(super) fn contains(lines: &[&str]) -> bool {
    let Some(start) = lines
        .iter()
        .rposition(|line| option_line(line).is_some_and(|option| option.number == 1))
    else {
        return false;
    };
    if unboxed(lines[start]).trim_start().starts_with('›') {
        return codex(lines, start);
    }
    let Some(top) = lines[..start]
        .iter()
        .rposition(|line| is_separator(unboxed(line)))
    else {
        return false;
    };
    unboxed(lines[top + 1]).trim_start().starts_with('❯')
        && lines[start + 1..]
            .iter()
            .any(|line| is_separator(unboxed(line)))
}

fn codex(lines: &[&str], start: usize) -> bool {
    // Command approvals can omit a hint and retain the model footer. Their
    // native option accelerators still identify them as menus.
    let (options, _) = numbered_options(lines, start);
    if options.iter().any(|option| {
        ["(y)", "(p)", "(esc)"]
            .iter()
            .any(|key| option.label.ends_with(key))
    }) {
        return false;
    }
    for line in &lines[start + 1..] {
        let text = unboxed(line).trim();
        let lower = text.to_ascii_lowercase();
        if is_separator(text)
            || lower.starts_with("press enter")
            || lower.starts_with("enter to ")
            || lower.starts_with("enter submit")
        {
            return false;
        }
        if lower.starts_with("gpt-") || text == "? for shortcuts" {
            return true;
        }
    }
    false
}
