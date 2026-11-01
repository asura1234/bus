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
    for key in [
        "y", "n", "2", "ctrl+c", "ctrl+d", "ctrl+z", "pageup", "alt+down", "+",
    ] {
        assert!(!input::encode_key(key, KeyboardProtocol::Legacy)
            .unwrap()
            .is_empty());
    }
    assert_eq!(
        input::paste("answer\n第二行", false),
        "answer\n第二行".as_bytes()
    );
    assert_eq!(input::paste("answer", true), b"\x1b[200~answer\x1b[201~");
    assert!(input::encode_key("unknown-key", KeyboardProtocol::Legacy).is_err());
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
