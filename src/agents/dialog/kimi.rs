use super::{
    indentation, is_separator, marked_label, title, unboxed, Dialog, DialogKind, DialogOption,
    MARKERS,
};

pub(super) fn parse(lines: &[&str]) -> Option<Dialog> {
    let first = lines.iter().rposition(|line| {
        unboxed(line)
            .trim_start()
            .trim_start_matches(MARKERS)
            .trim_start()
            .starts_with("Trust and continue")
    })?;
    let second = first + 1;
    if unboxed(lines.get(second)?)
        .trim_start()
        .trim_start_matches(MARKERS)
        .trim()
        != "Exit"
        || !lines[..first]
            .iter()
            .any(|line| unboxed(line).trim() == "Trust this folder?")
        || !lines[..first]
            .iter()
            .any(|line| unboxed(line).trim() == "↑↓ navigate · Enter select · Esc exit")
        || !lines[second + 1..].iter().all(|line| {
            let text = unboxed(line);
            text.trim().is_empty() || is_separator(text)
        })
    {
        return None;
    }
    let mut marked = [first, second]
        .into_iter()
        .filter(|index| marked_label(lines[*index]).is_some());
    let selected = marked.next()?;
    if marked.next().is_some() {
        return None;
    }
    let (column, _) = marked_label(lines[selected])?;
    let other = if selected == first { second } else { first };
    if indentation(unboxed(lines[other])) != column {
        return None;
    }
    let options = [first, second]
        .into_iter()
        .enumerate()
        .map(|(index, line)| DialogOption {
            number: index as u32 + 1,
            label: unboxed(lines[line])
                .trim_start()
                .trim_start_matches(MARKERS)
                .trim()
                .to_owned(),
            selected: line == selected,
            navigation_key: None,
            identity_label: None,
            free_text: false,
        })
        .collect();
    Some(Dialog {
        kind: DialogKind::Choice,
        input: None,
        text: title(&lines[..first]),
        options,
        hint: Some("↑↓ navigate · Enter select · Esc exit".into()),
        fingerprint_context: String::new(),
        tabs: Vec::new(),
    })
}
