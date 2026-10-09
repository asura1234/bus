use super::*;

fn width(ch: char) -> u16 {
    match ch {
        '\u{301}' => 0,
        '界' => 2,
        _ => 1,
    }
}

#[test]
fn word_and_url_bounds_use_the_callers_width_rules() {
    assert_eq!(word_bounds_at_column("ab", 2, |_| 2), Some((0, 3)));
    assert_eq!(word_bounds_at_column("a\u{301}界", 2, width), Some((0, 2)));
    assert_eq!(word_bounds_at_column("ab", 4, |_| 2), None);
    assert_eq!(
        url_at_column("界 https://example.com.", 4, width),
        Some("https://example.com")
    );
    assert_eq!(url_at_column("界 https://example.com.", 1, width), None);
}

#[test]
fn visible_cells_keep_zero_width_marks_and_pending_wrap_order() {
    let text = "a\u{301}界x\ny";
    let cells = visible_text_cells(text, 3, width);
    assert_eq!(
        cells
            .iter()
            .map(|cell| (cell.logical_col, cell.screen_row, cell.screen_col))
            .collect::<Vec<_>>(),
        vec![(0, 0, 0), (1, 0, 1), (1, 0, 1), (3, 1, 0), (0, 2, 0)]
    );
    assert_eq!(
        logical_cell_for_visible_cell(text, 3, 0, 1, width)
            .unwrap()
            .ch,
        '\u{301}'
    );
    assert_eq!(
        logical_cell_for_visible_cell(text, 3, 0, 2, width)
            .unwrap()
            .ch,
        '界'
    );
    assert_eq!(
        logical_cell_for_visible_cell(text, 3, 1, 0, width)
            .unwrap()
            .logical_col,
        3
    );
    assert!(visible_text_cells(text, 0, width).is_empty());
    assert!(logical_cell_for_visible_cell(text, 0, 0, 0, width).is_none());
}

#[test]
fn hit_test_columns_keep_saturating_width_arithmetic() {
    assert_eq!(
        word_bounds_at_column("ab", u16::MAX - 1, |_| u16::MAX),
        Some((0, u16::MAX - 1))
    );
    assert_eq!(word_bounds_at_column("ab", u16::MAX, |_| u16::MAX), None);
}
