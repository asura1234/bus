use super::Grid;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SelectionStep {
    Confirm,
    Move(&'static str),
}

/// Move one option at a time: menu rows may wrap or contain explanatory lines.
/// The caller reads the redraw after each move before deciding the next key.
pub(crate) fn selection_step(grid: &Grid, label: &str) -> Result<SelectionStep, String> {
    if label.trim().is_empty() || label.contains(['\r', '\n']) {
        return Err("Select needs one complete option label".into());
    }
    let rows = grid.rows_text();
    let targets: Vec<_> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| option_label(row).0 == label)
        .map(|(index, _)| index)
        .collect();
    let [target] = targets.as_slice() else {
        return Err(
            "Option label is missing or ambiguous; read the screen or send explicit keys".into(),
        );
    };
    let (focused, _) = selection_focus(grid)?;
    Ok(match target.cmp(&focused) {
        std::cmp::Ordering::Equal => SelectionStep::Confirm,
        std::cmp::Ordering::Less => SelectionStep::Move("up"),
        std::cmp::Ordering::Greater => SelectionStep::Move("down"),
    })
}

pub(crate) fn selection_focus(grid: &Grid) -> Result<(usize, String), String> {
    let rows = grid.rows_text();
    let focused: Vec<_> = rows
        .iter()
        .enumerate()
        .filter(|(index, row)| {
            option_label(row).1
                || grid.cells[*index]
                    .iter()
                    .any(|cell| cell.attrs.iter().any(|attr| attr == "inverse"))
        })
        .map(|(index, _)| index)
        .collect();
    let [focused] = focused.as_slice() else {
        return Err("Menu focus is not unique; read the screen or send explicit keys".into());
    };
    Ok((*focused, option_label(&rows[*focused]).0.to_owned()))
}

fn option_label(row: &str) -> (&str, bool) {
    let row = row.trim().trim_matches(['│', '┃']).trim();
    let selected = row.starts_with(['❯', '›', '>', '●', '◉', '▶', '→']);
    let row = if selected {
        row.chars()
            .next()
            .map_or(row, |c| &row[c.len_utf8()..])
            .trim_start()
    } else {
        row
    };
    let digits = row.bytes().take_while(u8::is_ascii_digit).count();
    let row = if digits > 0
        && row
            .as_bytes()
            .get(digits)
            .is_some_and(|c| matches!(c, b'.' | b')' | b':'))
    {
        row[digits + 1..].trim_start()
    } else {
        row
    };
    (row, selected)
}
