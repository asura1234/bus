use super::{
    indentation, is_separator, marked_label, title, unboxed, Dialog, DialogKind, DialogOption,
    MARKERS,
};

pub(super) fn parse(lines: &[&str]) -> Option<Dialog> {
    trust(lines).or_else(|| cache_expired(lines))
}

fn trust(lines: &[&str]) -> Option<Dialog> {
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

fn cache_expired(lines: &[&str]) -> Option<Dialog> {
    let first = lines.iter().rposition(|line| {
        unboxed(line)
            .trim_start()
            .trim_start_matches(MARKERS)
            .trim_start()
            .starts_with("Compact and continue")
    })?;
    let indices = [first, first + 1, first + 2, first + 3];
    let expected = [
        "Compact and continue",
        "Start a new session",
        "Continue as-is",
        "Don't ask me again",
    ];
    if !lines[..first].iter().any(|line| {
        unboxed(line)
            .trim()
            .starts_with("Cache expired — the next message re-sends the entire history")
    }) || !lines[..first]
        .iter()
        .any(|line| unboxed(line).trim() == "↑↓ navigate · Enter select · Esc cancel")
    {
        return None;
    }
    let label = |line: &str| {
        unboxed(line)
            .trim_start()
            .trim_start_matches(MARKERS)
            .trim_start()
            .split("  ")
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned()
    };
    if indices
        .iter()
        .zip(expected)
        .any(|(index, expected)| lines.get(*index).map(|line| label(line)) != Some(expected.into()))
    {
        return None;
    }
    let mut marked = indices
        .iter()
        .copied()
        .filter(|index| marked_label(lines[*index]).is_some());
    let selected = marked.next()?;
    if marked.next().is_some() {
        return None;
    }
    let (column, _) = marked_label(lines[selected])?;
    if indices
        .iter()
        .copied()
        .filter(|index| *index != selected)
        .any(|index| indentation(unboxed(lines[index])) != column)
    {
        return None;
    }
    let options = indices
        .into_iter()
        .enumerate()
        .map(|(index, line)| DialogOption {
            number: index as u32 + 1,
            label: label(lines[line]),
            selected: line == selected,
            navigation_key: None,
            identity_label: None,
            free_text: false,
        })
        .collect();
    Some(Dialog {
        kind: DialogKind::Choice,
        input: None,
        text: "Cache expired — the next message re-sends the entire history at full price.".into(),
        options,
        hint: Some("↑↓ navigate · Enter select · Esc cancel".into()),
        fingerprint_context: String::new(),
        tabs: Vec::new(),
    })
}
