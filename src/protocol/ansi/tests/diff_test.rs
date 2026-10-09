use super::*;

use crate::protocol::wire::{CellData, CursorState};

const WIDE_GRAPHEME: &str = "💡";

const HALFWIDTH_VOICED_KANA: &str = "ｶ\u{ff9e}";

fn make_cell(symbol: &str, fg: u32, bg: u32, modifier: u16) -> CellData {
    CellData {
        symbol: symbol.to_owned(),
        fg,
        bg,
        modifier,
        skip: false,
        hyperlink: None,
    }
}

fn make_skip_cell(symbol: &str, fg: u32, bg: u32, modifier: u16) -> CellData {
    let mut cell = make_cell(symbol, fg, bg, modifier);
    cell.skip = true;
    cell
}

fn make_frame(width: u16, height: u16, cells: Vec<CellData>) -> FrameData {
    FrameData {
        cells,
        width,
        height,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    }
}

fn linked_cell(symbol: &str, index: u32) -> CellData {
    let mut cell = make_cell(symbol, 0, 0, 0);
    cell.hyperlink = Some(index);
    cell
}

#[test]
fn color_to_sgr_fg_named_colors() {
    assert_eq!(color_to_sgr_fg(0x00_00_00_00), "39"); // Reset
    assert_eq!(color_to_sgr_fg(0x00_00_00_01), "30"); // Black
    assert_eq!(color_to_sgr_fg(0x00_00_00_02), "31"); // Red
    assert_eq!(color_to_sgr_fg(0x00_00_00_10), "97"); // White
}

#[test]
fn color_to_sgr_fg_indexed() {
    assert_eq!(color_to_sgr_fg(0x01_00_00_AB), "38;5;171");
}

#[test]
fn color_to_sgr_fg_rgb() {
    assert_eq!(color_to_sgr_fg(0x02_FF_80_40), "38;2;255;128;64");
}

#[test]
fn color_to_sgr_bg_named_colors() {
    assert_eq!(color_to_sgr_bg(0x00_00_00_00), "49"); // Reset
    assert_eq!(color_to_sgr_bg(0x00_00_00_01), "40"); // Black
    assert_eq!(color_to_sgr_bg(0x00_00_00_10), "107"); // White
}

#[test]
fn color_to_sgr_bg_rgb() {
    assert_eq!(color_to_sgr_bg(0x02_FF_80_40), "48;2;255;128;64");
}

#[test]
fn modifier_to_sgr_parts_bold() {
    let parts = modifier_to_sgr_parts(1); // BOLD
    assert!(parts.contains(&"1"));
}

#[test]
fn modifier_to_sgr_parts_italic() {
    let parts = modifier_to_sgr_parts(4); // ITALIC
    assert!(parts.contains(&"3"));
}

#[test]
fn modifier_to_sgr_parts_empty() {
    let parts = modifier_to_sgr_parts(0);
    assert!(parts.is_empty());
}

#[test]
fn build_sgr_produces_valid_sequence() {
    let sgr = build_sgr(0x00_00_00_02, 0x00_00_00_01, 1); // fg=Red, bg=Black, bold
    assert!(sgr.starts_with("\x1b["));
    assert!(sgr.ends_with("m"));
    assert!(sgr.contains("0")); // reset existing style first
    assert!(sgr.contains("1")); // bold
    assert!(sgr.contains("31")); // fg red
    assert!(sgr.contains("40")); // bg black
}

#[test]
fn build_sgr_resets_previous_modifiers_when_cell_is_plain() {
    assert_eq!(build_sgr(0x00_00_00_00, 0x00_00_00_00, 0), "\x1b[0;39;49m");
}

#[test]
fn build_sgr_preserves_curly_underline_style() {
    let modifier = crate::protocol::wire::modifier_to_u16(
        crate::protocol::wire::modifier_with_underline_style(
            ratatui::style::Modifier::UNDERLINED,
            3,
        ),
    );

    assert_eq!(
        build_sgr(0x00_00_00_00, 0x00_00_00_00, modifier),
        "\x1b[0;4:3;39;49m"
    );
}

#[test]
fn cells_equal_identical() {
    let a = make_cell("A", 2, 1, 0);
    let b = make_cell("A", 2, 1, 0);
    assert!(cells_equal(&a, &b));
}

#[test]
fn cells_equal_different_symbol() {
    let a = make_cell("A", 2, 1, 0);
    let b = make_cell("B", 2, 1, 0);
    assert!(!cells_equal(&a, &b));
}

#[test]
fn cells_equal_different_color() {
    let a = make_cell("A", 2, 1, 0);
    let b = make_cell("A", 3, 1, 0);
    assert!(!cells_equal(&a, &b));
}

#[test]
fn blit_frame_emits_osc8_for_linked_cells() {
    let mut frame = make_frame(
        3,
        1,
        vec![
            linked_cell("L", 0),
            linked_cell("i", 0),
            make_cell("!", 0, 0, 0),
        ],
    );
    frame.hyperlinks.push("https://example.com".to_owned());

    let mut output = Vec::new();
    blit_frame_to(&mut output, &frame, None);

    let output_str = String::from_utf8(output).unwrap();
    assert!(output_str.contains("\x1b]8;;https://example.com\x1b\\L"));
    assert!(output_str.contains('i'));
    assert!(output_str.contains("\x1b]8;;\x1b\\"));
}

#[test]
fn blit_frame_sanitizes_hyperlink_uris() {
    let mut frame = make_frame(1, 1, vec![linked_cell("L", 0)]);
    frame
        .hyperlinks
        .push("https://exa\x1b\x07mple.com".to_owned());

    let mut output = Vec::new();
    blit_frame_to(&mut output, &frame, None);

    let output_str = String::from_utf8(output).unwrap();
    assert!(output_str.contains("\x1b]8;;https://example.com\x1b\\L"));
}

#[test]
fn blit_frame_first_frame_produces_output() {
    let frame = make_frame(
        2,
        2,
        vec![
            make_cell("H", 0, 0, 0),
            make_cell("i", 0, 0, 0),
            make_cell("!", 0, 0, 0),
            make_cell(" ", 0, 0, 0),
        ],
    );

    let mut output = Vec::new();
    blit_frame_to(&mut output, &frame, None);

    let output_str = String::from_utf8(output).unwrap();
    // Full redraw should start with clear screen.
    assert!(
        output_str.contains("\x1b[2J"),
        "full redraw should clear screen"
    );
    assert!(
        output_str.contains('H') || output_str.contains('i'),
        "should contain cell content"
    );
}

#[test]
fn blit_frame_diff_only_writes_changed_cells() {
    let prev = make_frame(
        2,
        2,
        vec![
            make_cell("H", 0, 0, 0),
            make_cell("i", 0, 0, 0),
            make_cell("!", 0, 0, 0),
            make_cell(" ", 0, 0, 0),
        ],
    );

    // Only the first cell changed.
    let curr = make_frame(
        2,
        2,
        vec![
            make_cell("X", 0, 0, 0), // Changed
            make_cell("i", 0, 0, 0), // Same
            make_cell("!", 0, 0, 0), // Same
            make_cell(" ", 0, 0, 0), // Same
        ],
    );

    let mut output = Vec::new();
    blit_frame_to(&mut output, &curr, Some(&prev));

    let output_str = String::from_utf8(output).unwrap();
    // Diff should NOT clear the screen.
    assert!(
        !output_str.contains("\x1b[2J"),
        "diff should not clear screen"
    );
    // Should contain the changed cell content.
    assert!(output_str.contains('X'), "should contain changed cell 'X'");
}

#[test]
fn scroll_sized_ascii_shift_batches_changed_cells_by_row() {
    const WIDTH: u16 = 140;
    const HEIGHT: u16 = 50;
    let prev = make_frame(
        WIDTH,
        HEIGHT,
        vec![make_cell("A", 0, 0, 0); usize::from(WIDTH) * usize::from(HEIGHT)],
    );
    let curr = make_frame(
        WIDTH,
        HEIGHT,
        vec![make_cell("B", 0, 0, 0); usize::from(WIDTH) * usize::from(HEIGHT)],
    );

    let mut output = Vec::new();
    blit_frame_to(&mut output, &curr, Some(&prev));

    let cup_count = output.iter().filter(|&&byte| byte == b'H').count();
    assert!(
            cup_count <= usize::from(HEIGHT) + 2,
            "one dense scroll frame should need at most one CUP per row plus cursor anchors, got {cup_count}"
        );
    assert!(
            output.len() <= 16_290,
            "one dense scroll frame should stay below 25% of the 65,161-byte live baseline, got {} bytes",
            output.len()
        );
}

#[test]
fn encoder_forced_repaint_writes_all_cells_without_clearing() {
    let frame = make_frame(3, 2, vec![make_cell("A", 0, 0, 0); 6]);
    let mut encoder = BlitEncoder::new();
    let initial = encoder.encode(&frame, false);
    encoder.commit(frame.clone(), initial);

    let encoded = encoder.encode(&frame, true);
    let output = String::from_utf8(encoded.bytes).unwrap();

    assert!(!output.contains("\x1b[2J"));
    assert!(output.bytes().filter(|byte| *byte == b'A').count() >= 6);
}

#[test]
fn retained_patch_matches_full_diff_and_updates_the_encoder_baseline() {
    let previous = make_frame(
        4,
        2,
        vec![
            make_cell("a", 0, 0, 0),
            make_cell("b", 0, 0, 0),
            make_cell("c", 0, 0, 0),
            make_cell("d", 0, 0, 0),
            make_cell("e", 0, 0, 0),
            make_cell("f", 0, 0, 0),
            make_cell("g", 0, 0, 0),
            make_cell("h", 0, 0, 0),
        ],
    );
    let mut encoder = BlitEncoder::new();
    let initial = encoder.encode(&previous, false);
    encoder.commit(previous.clone(), initial);

    let rows = vec![PaneSurfacePatchRow {
        x: 0,
        y: 1,
        cells: vec![
            make_cell("E", 0, 0, 0),
            make_cell("f", 0, 0, 0),
            make_cell("G", 0, 0, 0),
            make_cell("h", 0, 0, 0),
        ],
    }];
    let cursor = Some(CursorState {
        x: 3,
        y: 1,
        visible: true,
        shape: 2,
    });
    let mut expected = previous;
    expected.cells[4..8].clone_from_slice(&rows[0].cells);
    expected.cursor = cursor.clone();

    let full_diff = encoder.encode(&expected, false);
    let patch = encoder
        .encode_patch(&rows, cursor.clone(), false)
        .expect("valid retained patch");
    assert_eq!(patch.bytes, full_diff.bytes);
    assert!(encoder.commit_patch(&rows, cursor, patch));
    assert_eq!(encoder.last_frame.as_ref(), Some(&expected));
}

#[test]
fn retained_patch_width_transition_matches_full_diff_with_following_cell() {
    let previous = make_frame(
        3,
        1,
        vec![
            make_cell("界", 0, 0, 0),
            make_cell("z", 0, 0, 0),
            make_cell("q", 0, 0, 0),
        ],
    );
    let mut encoder = BlitEncoder::new();
    let initial = encoder.encode(&previous, false);
    encoder.commit(previous.clone(), initial);

    let rows = vec![PaneSurfacePatchRow {
        x: 0,
        y: 0,
        cells: vec![make_cell("x", 0, 0, 0), make_cell("z", 0, 0, 0)],
    }];
    let mut expected = previous;
    expected.cells[0..2].clone_from_slice(&rows[0].cells);

    let full_diff = encoder.encode(&expected, false);
    let patch = encoder
        .encode_patch(&rows, None, false)
        .expect("valid retained patch");
    assert_eq!(patch.bytes, full_diff.bytes);
}

#[test]
fn retained_patch_rejects_overlapping_rows() {
    let frame = make_frame(
        3,
        1,
        vec![
            make_cell("a", 0, 0, 0),
            make_cell("b", 0, 0, 0),
            make_cell("c", 0, 0, 0),
        ],
    );
    let mut encoder = BlitEncoder::new();
    let initial = encoder.encode(&frame, false);
    encoder.commit(frame, initial);
    let rows = vec![
        PaneSurfacePatchRow {
            x: 0,
            y: 0,
            cells: vec![make_cell("A", 0, 0, 0), make_cell("B", 0, 0, 0)],
        },
        PaneSurfacePatchRow {
            x: 1,
            y: 0,
            cells: vec![make_cell("C", 0, 0, 0)],
        },
    ];

    assert!(encoder.encode_patch(&rows, None, false).is_none());
}

#[test]
fn full_redraw_skips_trailing_cells_covered_by_wide_graphemes() {
    let frame = FrameData {
        cells: vec![
            make_cell(WIDE_GRAPHEME, 0, 0, 0),
            make_cell(" ", 0, 0, 0),
            make_cell("Z", 0, 0, 0),
        ],
        width: 3,
        height: 1,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };

    let mut output = Vec::new();
    blit_frame_to(&mut output, &frame, None);
    let output_str = String::from_utf8(output).unwrap();

    assert!(output_str.contains("\x1b[1;1H"));
    assert!(!output_str.contains("\x1b[1;2H"));
    assert!(output_str.contains("\x1b[1;3H"));
}

#[test]
fn full_redraw_skips_trailing_cells_covered_by_halfwidth_voiced_kana() {
    let frame = FrameData {
        cells: vec![
            make_cell(HALFWIDTH_VOICED_KANA, 0, 0, 0),
            make_skip_cell(" ", 0, 0, 0),
            make_cell("Z", 0, 0, 0),
        ],
        width: 3,
        height: 1,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };

    let mut output = Vec::new();
    blit_frame_to(&mut output, &frame, None);
    let output_str = String::from_utf8(output).unwrap();

    assert!(output_str.contains("\x1b[1;1H"));
    assert!(!output_str.contains("\x1b[1;2H"));
    assert!(output_str.contains("\x1b[1;3H"));
}

#[test]
fn diff_redraw_reveals_cells_hidden_by_previous_wide_graphemes() {
    let prev = FrameData {
        cells: vec![
            make_cell(WIDE_GRAPHEME, 0, 0, 0),
            make_cell(" ", 0, 0, 0),
            make_cell("Z", 0, 0, 0),
        ],
        width: 3,
        height: 1,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };
    let curr = FrameData {
        cells: vec![
            make_cell("A", 0, 0, 0),
            make_cell(" ", 0, 0, 0),
            make_cell("Z", 0, 0, 0),
        ],
        width: 3,
        height: 1,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };

    let mut output = Vec::new();
    blit_frame_to(&mut output, &curr, Some(&prev));
    let output_str = String::from_utf8(output).unwrap();

    assert!(output_str.contains("\x1b[1;1H"));
    assert!(
        output_str.contains("\x1b[1;2H"),
        "cells hidden by a previous wide grapheme must be redrawn when they become visible"
    );
}

#[test]
fn diff_redraw_skips_new_trailing_cells_covered_by_wide_graphemes() {
    let prev = FrameData {
        cells: vec![
            make_cell("A", 0, 0, 0),
            make_cell("B", 0, 0, 0),
            make_cell("Z", 0, 0, 0),
        ],
        width: 3,
        height: 1,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };
    let curr = FrameData {
        cells: vec![
            make_cell(WIDE_GRAPHEME, 0, 0, 0),
            make_cell(" ", 0, 0, 0),
            make_cell("Z", 0, 0, 0),
        ],
        width: 3,
        height: 1,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };

    let mut output = Vec::new();
    blit_frame_to(&mut output, &curr, Some(&prev));
    let output_str = String::from_utf8(output).unwrap();

    assert!(output_str.contains("\x1b[1;1H"));
    assert!(!output_str.contains("\x1b[1;2H"));
}

#[test]
fn diff_redraw_reveals_cells_hidden_by_previous_halfwidth_voiced_kana() {
    let prev = FrameData {
        cells: vec![
            make_cell(HALFWIDTH_VOICED_KANA, 0, 0, 0),
            make_skip_cell(" ", 0, 0, 0),
            make_cell("Z", 0, 0, 0),
        ],
        width: 3,
        height: 1,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };
    let curr = FrameData {
        cells: vec![
            make_cell("A", 0, 0, 0),
            make_cell(" ", 0, 0, 0),
            make_cell("Z", 0, 0, 0),
        ],
        width: 3,
        height: 1,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };

    let mut output = Vec::new();
    blit_frame_to(&mut output, &curr, Some(&prev));
    let output_str = String::from_utf8(output).unwrap();

    assert!(output_str.contains("\x1b[1;1H"));
    assert!(
        output_str.contains("\x1b[1;2H"),
        "cells hidden by a previous halfwidth voiced kana must be redrawn when visible"
    );
}

#[path = "cursor_test.rs"]
mod cursor;
#[path = "sync_test.rs"]
mod sync;
