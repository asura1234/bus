use super::*;

#[test]
fn blit_frame_wraps_frame_in_synchronized_output() {
    let frame = make_frame(1, 1, vec![make_cell("A", 0, 0, 0)]);

    let mut output = Vec::new();
    blit_frame_to(&mut output, &frame, None);

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.starts_with("\x1b[?2026h\x1b[?25l"),
        "should begin synchronized output before frame writes"
    );
    let sync_end = output_str
        .find("\x1b[?2026l")
        .expect("should end synchronized output after frame writes");
    assert!(
        sync_end > 0,
        "should end synchronized output after frame writes"
    );
}

#[test]
fn blit_frame_begins_sync_before_hiding_cursor_after_visible_cursor_repeat() {
    let visible = FrameData {
        cells: vec![make_cell("A", 0, 0, 0); 9],
        width: 3,
        height: 3,
        cursor: Some(CursorState {
            x: 2,
            y: 1,
            visible: true,
            shape: 0,
        }),
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };
    let mut changed = visible.clone();
    changed.cells[0] = make_cell("B", 0, 0, 0);

    let mut last_visible_cursor = None;
    let mut last_cursor_shape = 0;
    let mut first_output = Vec::new();
    blit_frame_to_with_cursor_memory_and_policy(
        &mut first_output,
        &visible,
        None,
        &mut last_visible_cursor,
        &mut last_cursor_shape,
        true,
        false,
    );

    let mut second_output = Vec::new();
    blit_frame_to_with_cursor_memory_and_policy(
        &mut second_output,
        &changed,
        Some(&visible),
        &mut last_visible_cursor,
        &mut last_cursor_shape,
        true,
        false,
    );

    let second_output_str = std::str::from_utf8(&second_output).unwrap();
    assert!(
        second_output_str.starts_with("\x1b[?2026h\x1b[?25l"),
        "next frame should enter synchronized output before hiding the cursor"
    );

    let hide = second_output_str
        .find("\x1b[?25l")
        .expect("second frame should hide cursor before painting");
    let first_paint = second_output_str
        .find("\x1b[1;1H")
        .expect("second frame should paint changed cell");
    assert!(
        hide < first_paint,
        "cursor should still hide before painting"
    );

    first_output.extend_from_slice(&second_output);
    let combined = String::from_utf8(first_output).unwrap();
    assert!(
        combined.contains("\x1b[?2026l\x1b[2;3H\x1b[?25h\x1b[?2026h\x1b[?25l"),
        "post-sync cursor repeat should be followed by a synchronized cursor hide"
    );
}

#[test]
fn blit_frame_can_repeat_final_cursor_state_after_synchronized_output() {
    let frame = FrameData {
        cells: vec![make_cell("A", 0, 0, 0); 9],
        width: 3,
        height: 3,
        cursor: Some(CursorState {
            x: 2,
            y: 1,
            visible: true,
            shape: 0,
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
    let sync_end = output_str
        .find("\x1b[?2026l")
        .expect("should end synchronized output");
    let trailing_cursor = &output_str[sync_end + "\x1b[?2026l".len()..];
    assert_eq!(
        trailing_cursor, "\x1b[2;3H\x1b[?25h",
        "should expose only the final cursor state after synchronized output"
    );
}

#[test]
fn blit_frame_can_skip_final_cursor_state_after_synchronized_output() {
    let frame = FrameData {
        cells: vec![make_cell("A", 0, 0, 0); 9],
        width: 3,
        height: 3,
        cursor: Some(CursorState {
            x: 2,
            y: 1,
            visible: true,
            shape: 0,
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
        false,
        false,
    );

    let output_str = String::from_utf8(output).unwrap();
    let sync_end = output_str
        .find("\x1b[?2026l")
        .expect("should end synchronized output");
    let trailing_cursor = &output_str[sync_end + "\x1b[?2026l".len()..];
    assert_eq!(
        trailing_cursor, "",
        "should not expose a post-sync cursor repeat when the target terminal flickers on it"
    );
}

#[test]
fn blit_frame_repeats_explicit_hidden_cursor_anchor_after_synchronized_output() {
    let visible = FrameData {
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
    let hidden = FrameData {
        cells: vec![make_cell("B", 0, 0, 0); 9],
        width: 3,
        height: 3,
        cursor: Some(CursorState {
            x: 2,
            y: 1,
            visible: false,
            shape: 0,
        }),
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };
    let mut last_visible_cursor = None;
    let mut last_cursor_shape = 0;
    let mut output = Vec::new();

    blit_frame_to_with_cursor_memory_and_policy(
        &mut output,
        &visible,
        None,
        &mut last_visible_cursor,
        &mut last_cursor_shape,
        true,
        false,
    );
    output.clear();
    blit_frame_to_with_cursor_memory_and_policy(
        &mut output,
        &hidden,
        Some(&visible),
        &mut last_visible_cursor,
        &mut last_cursor_shape,
        true,
        false,
    );

    let output_str = String::from_utf8(output).unwrap();
    let sync_end = output_str
        .find("\x1b[?2026l")
        .expect("should end synchronized output");
    let trailing_cursor = &output_str[sync_end + "\x1b[?2026l".len()..];
    assert_eq!(
        trailing_cursor, "\x1b[2;3H\x1b[?25l",
        "should repeat the explicit hidden cursor position while preserving visibility"
    );
}
