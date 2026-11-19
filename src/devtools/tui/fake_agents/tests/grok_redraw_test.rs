use super::Grok;
use crate::agents::{detect_agent_with_osc, AgentKind, AgentState};
use crate::terminal::{emulator::GhosttyPaneTerminal, vt::Terminal};
use crate::utils::ids::PaneId;
use std::io::{self, Write};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Default)]
struct SharedOutput(Arc<Mutex<Vec<u8>>>);

impl SharedOutput {
    fn snapshot(&self) -> Vec<u8> {
        self.0.lock().unwrap().clone()
    }
}

impl Write for SharedOutput {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

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

#[test]
fn grok_single_idle_ctrl_c_arms_quit_without_exiting_and_second_exits() {
    let root = crate::utils::test_temp::unique_temp_path("bus-fake-grok-ctrl-c");
    let mut agent = Grok::new(root.clone(), "ctrl-c".into(), serde_json::json!({})).unwrap();
    let (tx, rx) = mpsc::channel();
    let output = SharedOutput::default();
    let mut writer = output.clone();
    let runner = std::thread::spawn(move || agent.read_loop_with(rx, &mut writer));

    // One Ctrl+C while idle can arrive from a developer interrupt racing turn
    // completion. Match Grok's two-press quit: stay alive, remain idle/usable,
    // accept the next prompt, and quit only on a second Ctrl+C while armed.
    tx.send(b"\x03".to_vec()).unwrap();
    std::thread::sleep(Duration::from_millis(120));
    assert!(
        !runner.is_finished(),
        "a single Ctrl+C while idle must arm quit instead of exiting fake Grok"
    );
    let armed_raw = String::from_utf8_lossy(&output.snapshot()).into_owned();
    let armed_frame = armed_raw.rsplit("\x1b[?2026h\x1b[H\x1b[2J").next().unwrap();
    assert!(
        armed_frame.contains("Ctrl+c:press again to quit"),
        "expected armed quit footer: {armed_frame}"
    );
    let clean_armed = armed_frame
        .replace("\x1b]0;grok\x07\x1b]9;4;0;0\x07", "")
        .replace("\x1b[?2026l", "")
        .replace("\r\n", "\n");
    assert_eq!(
        detect_agent_with_osc(Some(AgentKind::Grok), &clean_armed, "grok", "4;0;0").state,
        AgentState::Idle,
        "armed-quit screen must stay Idle: {clean_armed}"
    );
    assert!(
        !crate::terminal::driver::has_input_controls(&clean_armed),
        "armed-quit screen must not trigger dialog blocker: {clean_armed}"
    );

    // Typing a follow-up prompt disarms quit and runs normally.
    tx.send(b"hello\r".to_vec()).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        if String::from_utf8_lossy(&output.snapshot()).contains("fake reply: hello") {
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    assert!(
        String::from_utf8_lossy(&output.snapshot()).contains("fake reply: hello"),
        "fake Grok must stay alive after one idle Ctrl+C and answer the next prompt"
    );

    // Two Ctrl+C presses while idle exit cleanly.
    tx.send(b"\x03".to_vec()).unwrap();
    std::thread::sleep(Duration::from_millis(80));
    assert!(
        !runner.is_finished(),
        "first Ctrl+C after disarming must re-arm rather than exit"
    );
    tx.send(b"\x03".to_vec()).unwrap();
    let exit_deadline = std::time::Instant::now() + Duration::from_secs(2);
    while !runner.is_finished() && std::time::Instant::now() < exit_deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        runner.is_finished(),
        "second Ctrl+C while quit is armed must exit fake Grok"
    );
    runner.join().unwrap().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn grok_idle_read_loop_does_not_redraw_unchanged_screen() {
    let root = crate::utils::test_temp::unique_temp_path("bus-fake-grok-idle-redraw");
    let mut agent = Grok::new(root.clone(), "idle-redraw".into(), serde_json::json!({})).unwrap();
    let (tx, rx) = mpsc::channel();
    let output = SharedOutput::default();
    let mut writer = output.clone();
    let runner = std::thread::spawn(move || agent.read_loop_with(rx, &mut writer));

    // Wait across multiple 50ms poll timeouts with no input or state changes.
    std::thread::sleep(Duration::from_millis(260));
    drop(tx);
    runner.join().unwrap().unwrap();

    let raw = output.snapshot();
    let frames = raw
        .windows(b"\x1b[?2026h".len())
        .filter(|window| *window == b"\x1b[?2026h")
        .count();
    assert_eq!(
        frames, 1,
        "idle fake Grok must draw once instead of churning PTY redraws every 50ms (saw {frames} frames)"
    );
    std::fs::remove_dir_all(root).unwrap();
}
