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
    let (focused, _) = selection_focus_for_label(grid, Some(label))?;
    Ok(match target.cmp(&focused) {
        std::cmp::Ordering::Equal => SelectionStep::Confirm,
        std::cmp::Ordering::Less => SelectionStep::Move("up"),
        std::cmp::Ordering::Greater => SelectionStep::Move("down"),
    })
}

#[cfg(test)]
pub(crate) fn selection_focus(grid: &Grid) -> Result<(usize, String), String> {
    selection_focus_for_label(grid, None)
}

pub(crate) fn selection_focus_for_label(
    grid: &Grid,
    label: Option<&str>,
) -> Result<(usize, String), String> {
    let rows = grid.rows_text();
    let range = if let Some(label) = label {
        let targets: Vec<_> = rows
            .iter()
            .enumerate()
            .filter(|(_, text)| option_label(text).0 == label)
            .map(|(row, _)| row)
            .collect();
        let [target] = targets.as_slice() else {
            return Err(
                "Option label is missing or ambiguous; read the screen or send explicit keys"
                    .into(),
            );
        };
        menu_block(grid, &rows, *target)
    } else {
        0..rows.len()
    };
    let marked: Vec<_> = range
        .clone()
        .filter(|row| option_label(&rows[*row]).1)
        .collect();
    let inverse: Vec<_> = range
        .filter(|row| inverse_label_selected(grid, *row, &rows[*row]))
        .collect();
    // Without a requested menu, conflicting blocks do not establish unique focus.
    if label.is_none() && !marked.is_empty() && inverse.iter().any(|row| !marked.contains(row)) {
        return Err("Menu focus is not unique; read the screen or send explicit keys".into());
    }
    let focused = if marked.is_empty() { inverse } else { marked };
    let [focused] = focused.as_slice() else {
        return Err("Menu focus is not unique; read the screen or send explicit keys".into());
    };
    Ok((*focused, option_label(&rows[*focused]).0.to_owned()))
}

fn menu_block(grid: &Grid, rows: &[String], target: usize) -> std::ops::Range<usize> {
    let column = option_column(grid, target, &rows[target]);
    let belongs =
        |row: usize| rows[row].trim().is_empty() || option_column(grid, row, &rows[row]) == column;
    let mut start = target;
    while start > 0 && belongs(start - 1) {
        start -= 1;
    }
    let mut end = target + 1;
    while end < rows.len() && belongs(end) {
        end += 1;
    }
    start..end
}

fn option_column(grid: &Grid, row: usize, text: &str) -> Option<u16> {
    let body = option_body(text).0;
    if body.is_empty() {
        return None;
    }
    let start = body.as_ptr() as usize - text.as_ptr() as usize;
    grid.row_with_columns(row).1.get(start).copied()
}

fn inverse_label_selected(grid: &Grid, row: usize, text: &str) -> bool {
    let label = option_label(text).0;
    if label.is_empty() {
        return false;
    }
    // option_label returns a slice of this row; convert its byte span to cells.
    let start = label.as_ptr() as usize - text.as_ptr() as usize;
    let (_, columns) = grid.row_with_columns(row);
    label.char_indices().all(|(offset, _)| {
        columns
            .get(start + offset)
            .and_then(|col| grid.cells[row].get(usize::from(*col)))
            .is_some_and(|cell| cell.attrs.iter().any(|attr| attr == "inverse"))
    })
}

/// Physical row relocation does not change an option's logical identity.
/// The occurrence distinguishes equal intermediate labels without using screen coordinates.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SelectionIdentity {
    label: String,
    pub(crate) occurrence: usize,
}

pub(crate) fn selection_identity(
    grid: &Grid,
    label: Option<&str>,
) -> Result<SelectionIdentity, String> {
    let (row, label) = selection_focus_for_label(grid, label)?;
    let occurrence = grid.rows_text()[..row]
        .iter()
        .filter(|text| option_label(text).0 == label)
        .count();
    Ok(SelectionIdentity { label, occurrence })
}

fn option_body(row: &str) -> (&str, bool) {
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
    (row, selected)
}

fn option_label(row: &str) -> (&str, bool) {
    let (row, selected) = option_body(row);
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
