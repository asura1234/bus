use super::*;
use crate::protocol::keys::KeyboardProtocol;

fn menu(text: &str) -> Grid {
    let mut grid = Grid::blank(80, 12);
    for (row, text) in text.lines().enumerate() {
        for (col, c) in text.chars().enumerate() {
            grid.cells[row][col].symbol = c.to_string();
        }
    }
    grid
}

#[test]
fn select_moves_before_confirming_the_requested_row() {
    let first =
        menu("Update available\n❯ 1. Install update now\n  2. Continue with current version");
    assert_eq!(
        selection_step(&first, "Continue with current version"),
        Ok(SelectionStep::Move("down"))
    );
    let next =
        menu("Update available\n  1. Install update now\n❯ 2. Continue with current version");
    assert_eq!(
        selection_step(&next, "Continue with current version"),
        Ok(SelectionStep::Confirm)
    );
}

#[test]
fn select_does_not_depend_on_option_numbers_or_physical_row_distance() {
    let grid = menu("● Compact and continue\n  explanatory text\n\n  Continue as-is");
    assert_eq!(
        selection_step(&grid, "Continue as-is"),
        Ok(SelectionStep::Move("down"))
    );
    assert!(selection_step(&grid, "Continue").is_err());
    assert!(selection_step(&menu("❯ Yes\n  Yes"), "Yes").is_err());
    assert!(selection_step(&menu("  Yes\n  No"), "No").is_err());
}

#[test]
fn driver_encodes_shell_answers_control_keys_and_arbitrary_text() {
    let encode = |name: &str| {
        input::encode_key_with(name, |key| {
            crate::protocol::keys::encode_terminal_key(key, KeyboardProtocol::Legacy)
        })
    };
    for (key, expected) in [
        ("y", &b"y"[..]),
        ("n", &b"n"[..]),
        ("2", &b"2"[..]),
        ("ctrl+c", &b"\x03"[..]),
        ("ctrl+d", &b"\x04"[..]),
        ("ctrl+z", &b"\x1a"[..]),
        ("pageup", &b"\x1b[5~"[..]),
        ("alt+down", &b"\x1b[1;3B"[..]),
        ("+", &b"+"[..]),
    ] {
        assert_eq!(encode(key).unwrap(), expected, "{key}");
    }
    assert_eq!(
        input::paste("answer\n第二行", false),
        "answer\n第二行".as_bytes()
    );
    assert_eq!(input::paste("answer", true), b"\x1b[200~answer\x1b[201~");
    assert!(encode("unknown-key").is_err());
}

#[tokio::test]
async fn production_driver_observes_shell_without_an_agent_or_dialog_parser() {
    let (runtime, _) = crate::terminal::TerminalRuntime::test_with_channel(80, 12);
    runtime.test_process_pty_bytes(b"\x1b[31m[oh-my-zsh]\x1b[0m Would you like to update? [Y/n] ");
    let (grid, revision) = observe(&runtime).unwrap();
    assert!(grid.text().contains("Would you like to update? [Y/n]"));
    assert_eq!(grid.rows, 12);
    assert_eq!(revision, runtime.content_seq());
    assert_ne!(grid.cells[0][0].fg, "default");
    runtime.test_process_pty_bytes(b"\x1b[?2026hpartial redraw");
    assert!(observe(&runtime).is_err());
}

#[tokio::test]
async fn observing_terminal_preserves_unpublished_dirty_rows() {
    use crate::terminal::emulator::TerminalDirtyPatchOutcome;
    fn changed_runtime() -> crate::terminal::TerminalRuntime {
        let (runtime, _writes) = crate::terminal::TerminalRuntime::test_with_channel(20, 3);
        runtime.test_process_pty_bytes(b"OLD");
        assert!(matches!(
            runtime.collect_dirty_patch(20, 3),
            TerminalDirtyPatchOutcome::Patch(_)
        ));
        runtime.test_process_pty_bytes(b"\rNEW");
        runtime
    }
    let baseline = changed_runtime();
    let control = baseline.collect_dirty_patch(20, 3);
    assert!(matches!(&control, TerminalDirtyPatchOutcome::Patch(patch) if !patch.rows.is_empty()));
    let runtime = changed_runtime();
    let (grid, _) = observe(&runtime).unwrap();
    assert!(grid.text().contains("NEW"));
    let after_observation = runtime.collect_dirty_patch(20, 3);
    let TerminalDirtyPatchOutcome::Patch(patch) = after_observation else {
        panic!("read-only observation must retain the unpublished patch");
    };
    assert!(patch.rows.iter().any(|(_, cells)| cells
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>()
        .contains("NEW")));
}

#[tokio::test]
async fn inverse_only_menu_focus_survives_terminal_observation() {
    let (runtime, _writes) = crate::terminal::TerminalRuntime::test_with_channel(20, 3);
    runtime.test_process_pty_bytes(b"\x1b[7mYes\x1b[0m\r\nNo");
    let (grid, _) = observe(&runtime).unwrap();
    assert_eq!(
        selection_step(&grid, "Yes"),
        Ok(SelectionStep::Confirm),
        "a visibly inverse-selected row must remain selectable after production observation"
    );
}

#[tokio::test]
async fn production_driver_preserves_cells_links_wide_text_and_cursor() {
    let (runtime, _) = crate::terminal::TerminalRuntime::test_with_channel(20, 3);
    runtime.test_process_pty_bytes(
        "\x1b[1;3;4;38;2;1;2;3m界\x1b[0m\x1b]8;;https://example.test\x1b\\link\x1b]8;;\x1b\\"
            .as_bytes(),
    );
    let (grid, _) = observe(&runtime).unwrap();
    assert_eq!(grid.row_text(0), "界link");
    assert_eq!(grid.cells[0][1].symbol, "");
    assert_eq!(grid.cells[0][0].fg, "#010203");
    assert_eq!(grid.cells[0][0].attrs, ["bold", "italic", "underline"]);
    assert_eq!(
        grid.cells[0][2].link.as_deref(),
        Some("https://example.test")
    );
    let cursor = grid.cursor.unwrap();
    assert_eq!((cursor.row, cursor.col, cursor.visible), (0, 6, true));
    runtime.test_process_pty_bytes(b"\x1b[?25l");
    assert!(!observe(&runtime).unwrap().0.cursor.unwrap().visible);
}
