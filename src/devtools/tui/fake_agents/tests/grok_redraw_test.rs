use super::Grok;
use crate::terminal::{emulator::GhosttyPaneTerminal, vt::Terminal};
use crate::utils::ids::PaneId;

#[test]
fn grok_redraw_hides_the_partial_launch_composer_until_its_footer_arrives() {
    let root = crate::utils::test_temp::unique_temp_path("bus-fake-grok-redraw");
    let mut agent = Grok::new(root.clone(), "redraw".into(), serde_json::json!({})).unwrap();
    agent.draft = "Bus: launch instructions\n\nFollow these instructions.".into();
    let mut frame = Vec::new();
    agent.draw_to(&mut frame).unwrap();
    let footer = frame
        .windows(b"Shift+Tab:mode".len())
        .position(|bytes| bytes == b"Shift+Tab:mode")
        .unwrap();
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let pane = GhosttyPaneTerminal::new(Terminal::new(111, 40, 100).unwrap(), tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    // A PTY read can end between the pasted chip and the footer. That is an
    // unfinished repaint, not a complete screen that refuses the submit key.
    pane.process_pty_bytes(pane_id, 0, &frame[..footer], &tx, |_| None);
    assert!(
        pane.active_ansi_if_complete().is_none(),
        "a partial Grok repaint must not reach the native submission guard"
    );
    pane.process_pty_bytes(pane_id, 0, &frame[footer..], &tx, |_| None);
    let complete = pane.active_ansi_if_complete().unwrap();
    assert!(complete.contains("[Pasted: 3 lines]"));
    assert!(complete.contains("Shift+Tab:mode"));
    std::fs::remove_dir_all(root).unwrap();
}
