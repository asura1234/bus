//! Cursor's native startup workspace-trust chooser.
use super::{
    is_separator, marked_label, title, unboxed, Dialog, DialogKind, DialogOption, MARKERS,
};

pub(super) fn startup_trust(lines: &[&str]) -> Option<Dialog> {
    let hint = "Use arrow keys to navigate, Enter to select, or press the key shown";
    let footer = lines
        .iter()
        .rposition(|line| unboxed(line).trim() == hint)?;
    if !lines[footer + 1..].iter().all(|line| {
        let text = unboxed(line).trim();
        text.is_empty() || is_separator(text)
    }) {
        return None;
    }
    let first = lines[..footer].iter().rposition(|line| {
        unboxed(line)
            .trim_start()
            .trim_start_matches(MARKERS)
            .trim()
            == "[a] Trust this workspace"
    })?;
    let second = first + 1;
    if second >= footer
        || unboxed(lines[second])
            .trim_start()
            .trim_start_matches(MARKERS)
            .trim()
            != "[q] Quit"
        || !lines[..first].iter().any(|line| {
            unboxed(line).trim().trim_start_matches('⚠').trim() == "Workspace Trust Required"
        })
        || !lines[..first]
            .iter()
            .any(|line| unboxed(line).trim() == "Do you trust the contents of this directory?")
        || lines[second + 1..footer]
            .iter()
            .any(|line| !unboxed(line).trim().is_empty())
    {
        return None;
    }
    let selected: Vec<_> = [first, second]
        .into_iter()
        .filter(|index| marked_label(lines[*index]).is_some())
        .collect();
    if selected.len() != 1 {
        return None;
    }
    let options = [(first, "Trust this workspace", "a"), (second, "Quit", "q")]
        .into_iter()
        .enumerate()
        .map(|(index, (line, label, key))| DialogOption {
            number: index as u32 + 1,
            label: label.into(),
            selected: line == selected[0],
            free_text: false,
            navigation_key: Some(key),
            identity_label: None,
        })
        .collect();
    Some(Dialog {
        kind: DialogKind::Choice,
        input: None,
        text: title(&lines[..first]),
        options,
        hint: Some(hint.into()),
        fingerprint_context: String::new(),
        tabs: Vec::new(),
    })
}
