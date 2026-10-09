use super::*;

#[test]
fn blit_frame_hides_cursor_before_full_redraw_writes() {
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
    assert!(
        output_str.starts_with("\x1b[?2026h\x1b[?25l"),
        "should hide cursor inside synchronized frame painting during full redraw"
    );
}

#[test]
fn blit_frame_hides_cursor_before_diff_writes() {
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
    assert!(
        output_str.starts_with("\x1b[?2026h\x1b[?25l"),
        "should hide cursor inside synchronized frame painting during diff"
    );
}

#[test]
fn drawn_cursor_reverses_visible_cursor_cell() {
    let frame = FrameData {
        cells: vec![make_cell("A", 0, 0, 0); 9],
        width: 3,
        height: 3,
        cursor: Some(CursorState {
            x: 2,
            y: 1,
            visible: true,
            shape: 6,
        }),
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };
    let drawn = frame_with_drawn_cursor(frame.clone());

    assert_eq!(drawn.cells[5].modifier, REVERSED_MODIFIER);
    assert_eq!(frame.cells[5].modifier, 0);

    let encoded = BlitEncoder::new().encode_with_suppressed_visible_cursor(&drawn, false);
    let output_str = String::from_utf8(encoded.bytes).unwrap();

    assert!(
        output_str.contains("\x1b[2;3H\x1b[6 q\x1b[?25l"),
        "drawn cursor mode should park the host cursor hidden at the focused cursor position"
    );
    assert!(
        !output_str.contains("\x1b[?25h"),
        "drawn cursor mode should not show the host cursor"
    );
    assert!(
        output_str.contains("\x1b[0;7;39;49mA"),
        "drawn cursor should be emitted as reverse-video cell content"
    );
}

#[test]
fn drawn_cursor_ignores_hidden_cursor() {
    let frame = FrameData {
        cells: vec![make_cell("A", 0, 0, 0)],
        width: 1,
        height: 1,
        cursor: Some(CursorState {
            x: 0,
            y: 0,
            visible: false,
            shape: 0,
        }),
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };

    assert_eq!(frame_with_drawn_cursor(frame.clone()), frame);
}

#[test]
fn blit_frame_emits_cursor_shape_before_visibility_without_touching_ime_anchor() {
    let frame = FrameData {
        cells: vec![make_cell("A", 0, 0, 0)],
        width: 1,
        height: 1,
        cursor: Some(CursorState {
            x: 0,
            y: 0,
            visible: true,
            shape: 6,
        }),
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };

    let mut last_visible_cursor = None;
    let mut last_cursor_shape = 0;
    let mut output = Vec::new();
    blit_frame_to_with_cursor_memory_and_policy(
        &mut output,
        &frame,
        None,
        &mut last_visible_cursor,
        &mut last_cursor_shape,
        true,
        false,
    );

    let output_str = String::from_utf8(output).unwrap();
    let final_cursor = output_str
        .find("\x1b[1;1H\x1b[6 q\x1b[?25h")
        .expect("should set cursor shape before showing cursor");
    let sync_end = output_str
        .find("\x1b[?2026l")
        .expect("should end synchronized output");
    assert!(
        final_cursor < sync_end,
        "shape should be part of the synchronized final cursor state"
    );
    let trailing_cursor = &output_str[sync_end + "\x1b[?2026l".len()..];
    assert_eq!(
        trailing_cursor, "\x1b[1;1H\x1b[?25h",
        "IME anchor update should preserve the existing position/visibility-only contract"
    );
}

#[test]
fn retained_patch_preserves_the_client_drawn_cursor_overlay() {
    let mut previous = make_frame(
        3,
        1,
        vec![
            make_cell("a", 0, 0, 0),
            make_cell("b", 0, 0, 0),
            make_cell("c", 0, 0, 0),
        ],
    );
    previous.cursor = Some(CursorState {
        x: 0,
        y: 0,
        visible: true,
        shape: 0,
    });
    let previous_drawn = frame_with_drawn_cursor(previous.clone());
    let mut encoder = BlitEncoder::new();
    let initial = encoder.encode_with_suppressed_visible_cursor(&previous_drawn, false);
    encoder.commit(previous_drawn, initial);

    let rows = vec![PaneSurfacePatchRow {
        x: 0,
        y: 0,
        cells: vec![
            make_cell("A", 0, 0, 0),
            make_cell("b", 0, 0, 0),
            make_cell("c", 0, 0, 0),
        ],
    }];
    let cursor = Some(CursorState {
        x: 1,
        y: 0,
        visible: true,
        shape: 0,
    });
    let drawn_rows = encoder
        .patch_rows_with_drawn_cursor(&rows, cursor.as_ref())
        .expect("drawn cursor patch rows");
    let mut expected = previous;
    expected.cells[0..3].clone_from_slice(&rows[0].cells);
    expected.cursor = cursor.clone();
    let expected = frame_with_drawn_cursor(expected);

    let full_diff = encoder.encode_with_suppressed_visible_cursor(&expected, false);
    let patch = encoder
        .encode_patch(&drawn_rows, cursor.clone(), true)
        .expect("valid drawn cursor patch");
    assert_eq!(patch.bytes, full_diff.bytes);
    assert!(encoder.commit_patch(&drawn_rows, cursor, patch));
    assert_eq!(encoder.last_frame.as_ref(), Some(&expected));
}

#[test]
fn blit_frame_positions_cursor() {
    let frame = FrameData {
        cells: vec![make_cell("A", 0, 0, 0)],
        width: 1,
        height: 1,
        cursor: Some(CursorState {
            x: 0,
            y: 0,
            visible: true,
            shape: 0,
        }),
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };

    let mut output = Vec::new();
    blit_frame_to(&mut output, &frame, None);

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains("\x1b[1;1H"),
        "should position cursor at (1,1)"
    );
}

#[test]
fn blit_frame_hides_cursor_when_invisible() {
    let frame = FrameData {
        cells: vec![make_cell("A", 0, 0, 0)],
        width: 1,
        height: 1,
        cursor: Some(CursorState {
            x: 0,
            y: 0,
            visible: false,
            shape: 0,
        }),
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };

    let mut output = Vec::new();
    blit_frame_to(&mut output, &frame, None);

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains("\x1b[?25l"),
        "should hide cursor when invisible"
    );
}

#[test]
fn blit_frame_no_cursor_hides_cursor() {
    let frame = FrameData {
        cells: vec![make_cell("A", 0, 0, 0)],
        width: 1,
        height: 1,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };

    let mut output = Vec::new();
    blit_frame_to(&mut output, &frame, None);

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains("\x1b[?25l"),
        "should hide cursor when no cursor state"
    );
}

#[test]
fn blit_frame_restores_cursor_visibility() {
    // First frame: cursor hidden.
    let prev = FrameData {
        cells: vec![make_cell("A", 0, 0, 0)],
        width: 1,
        height: 1,
        cursor: Some(CursorState {
            x: 0,
            y: 0,
            visible: false,
            shape: 0,
        }),
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };

    let mut output = Vec::new();
    blit_frame_to(&mut output, &prev, None);
    assert!(
        String::from_utf8(output).unwrap().contains("\x1b[?25l"),
        "first frame should hide cursor"
    );

    // Second frame: cursor visible — should restore visibility.
    let curr = FrameData {
        cells: vec![make_cell("B", 0, 0, 0)],
        width: 1,
        height: 1,
        cursor: Some(CursorState {
            x: 0,
            y: 0,
            visible: true,
            shape: 0,
        }),
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };

    let mut output = Vec::new();
    blit_frame_to(&mut output, &curr, Some(&prev));
    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains("\x1b[?25h"),
        "second frame should restore cursor visibility with ?25h"
    );
    assert!(
        output_str.contains("\x1b[1;1H"),
        "should position cursor before showing it"
    );
}

#[test]
fn blit_frame_positions_cursor_before_showing_it() {
    let prev = FrameData {
        cells: vec![make_cell("A", 0, 0, 0); 9],
        width: 3,
        height: 3,
        cursor: Some(CursorState {
            x: 0,
            y: 0,
            visible: true,
            shape: 0,
        }),
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };
    let mut curr = prev.clone();
    curr.cells[0] = make_cell("B", 0, 0, 0);
    curr.cursor = Some(CursorState {
        x: 2,
        y: 2,
        visible: true,
        shape: 0,
    });

    let mut output = Vec::new();
    blit_frame_to(&mut output, &curr, Some(&prev));
    let output_str = String::from_utf8(output).unwrap();
    let final_move = output_str
        .rfind("\x1b[3;3H")
        .expect("should move cursor to final position");
    let show = output_str
        .rfind("\x1b[?25h")
        .expect("should show cursor after positioning it");

    assert!(
        final_move < show,
        "should move cursor to final position before showing it"
    );
}

#[test]
fn blit_frame_parks_hidden_cursor_at_last_visible_position() {
    let visible = FrameData {
        cells: vec![make_cell("A", 0, 0, 0); 9],
        width: 3,
        height: 3,
        cursor: Some(CursorState {
            x: 1,
            y: 1,
            visible: true,
            shape: 0,
        }),
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };
    let hidden = FrameData {
        cells: vec![make_cell("B", 0, 0, 0); 9],
        width: 3,
        height: 3,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };
    let mut last_visible_cursor = None;
    let mut last_cursor_shape = 0;
    let mut output = Vec::new();

    blit_frame_to_with_cursor_memory(
        &mut output,
        &visible,
        None,
        &mut last_visible_cursor,
        &mut last_cursor_shape,
        false,
    );
    output.clear();
    blit_frame_to_with_cursor_memory(
        &mut output,
        &hidden,
        Some(&visible),
        &mut last_visible_cursor,
        &mut last_cursor_shape,
        false,
    );

    let output_str = String::from_utf8(output).unwrap();
    let park = output_str
        .rfind("\x1b[2;2H")
        .expect("should park hidden cursor at last visible position");
    let hide = output_str
        .rfind("\x1b[?25l")
        .expect("should keep hidden cursor hidden");
    assert!(park < hide, "should park cursor before hiding it");
}

#[test]
fn blit_frame_parks_hidden_cursor_at_bottom_right_without_history() {
    let frame = FrameData {
        cells: vec![make_cell("A", 0, 0, 0); 6],
        width: 3,
        height: 2,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };
    let mut last_visible_cursor = None;
    let mut last_cursor_shape = 0;
    let mut output = Vec::new();

    blit_frame_to_with_cursor_memory(
        &mut output,
        &frame,
        None,
        &mut last_visible_cursor,
        &mut last_cursor_shape,
        false,
    );

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains("\x1b[2;3H\x1b[?25l"),
        "should park hidden cursor at bottom-right before ending the frame"
    );
}

#[test]
fn blit_frame_hides_previous_visible_cursor_when_next_frame_has_none() {
    let prev = FrameData {
        cells: vec![make_cell("A", 0, 0, 0)],
        width: 1,
        height: 1,
        cursor: Some(CursorState {
            x: 0,
            y: 0,
            visible: true,
            shape: 0,
        }),
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };
    let curr = FrameData {
        cells: vec![make_cell("B", 0, 0, 0)],
        width: 1,
        height: 1,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };

    let mut output = Vec::new();
    blit_frame_to(&mut output, &curr, Some(&prev));

    assert!(
        String::from_utf8(output).unwrap().contains("\x1b[?25l"),
        "diff redraw should hide a previously visible cursor when the next frame has none"
    );
}
