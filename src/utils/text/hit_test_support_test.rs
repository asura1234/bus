use super::text_cells;

pub(crate) fn text_in_cell_range(row: &str, start_col: u16, end_col: u16) -> String {
    text_cells(row)
        .into_iter()
        .filter(|cell| cell.start_col >= start_col && cell.end_col <= end_col)
        .map(|cell| cell.ch)
        .collect()
}
