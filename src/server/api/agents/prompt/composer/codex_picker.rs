//! Native Codex picker chrome outside the editable composer.
fn rows(screen: &str) -> Vec<String> {
    screen
        .lines()
        .map(|line| {
            super::styled_chars(line)
                .iter()
                .map(|cell| cell.character)
                .collect::<String>()
                .trim()
                .to_owned()
        })
        .collect()
}

pub(super) fn mentions(screen: &str) -> bool {
    let rows = rows(screen);
    let Some(prompt) = rows.iter().rposition(|row| row.starts_with('›')) else {
        return false;
    };
    let Some(footer) = rows[..prompt].iter().rposition(|row| !row.is_empty()) else {
        return false;
    };
    rows[footer].starts_with("enter/tab insert")
        && rows[footer].contains("esc close")
        && rows[..footer]
            .iter()
            .rev()
            .take(24)
            .any(|row| row == "Mentions")
}

pub(super) fn active(screen: &str) -> bool {
    mentions(screen)
        || rows(screen)
            .iter()
            .rev()
            .find(|row| !row.is_empty())
            .is_some_and(|row| row.starts_with("reverse-i-search:"))
}
