use super::*;
use crate::protocol::wire::CellData;
use std::{
    sync::{mpsc, Condvar, Mutex},
    time::Duration,
};

#[derive(Default)]
struct Capture {
    open: bool,
    bytes: Vec<u8>,
    flushes: usize,
}
struct SlowWriter {
    state: Arc<(Mutex<Capture>, Condvar)>,
    entered: Option<mpsc::Sender<()>>,
    fail_flush: bool,
}
impl Write for SlowWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Some(tx) = self.entered.take() {
            tx.send(()).unwrap();
        }
        let (lock, wake) = &*self.state;
        let mut state = lock.lock().unwrap();
        while !state.open {
            state = wake.wait(state).unwrap();
        }
        let count = bytes.len().min(7); // force real write_all partial-write handling
        state.bytes.extend_from_slice(&bytes[..count]);
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.state.0.lock().unwrap().flushes += 1;
        if self.fail_flush {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "probe flush"))
        } else {
            Ok(())
        }
    }
}
fn frame(symbol: &str, width: u16) -> FrameData {
    FrameData {
        width,
        height: 1,
        cells: vec![
            CellData {
                symbol: symbol.into(),
                fg: 0,
                bg: 0,
                modifier: 0,
                skip: false,
                hyperlink: None
            };
            width.into()
        ],
        cursor: None,
        hyperlinks: vec![],
        graphics: vec![],
    }
}
fn slow(
    fail_flush: bool,
) -> (
    TerminalOutput,
    Arc<(Mutex<Capture>, Condvar)>,
    mpsc::Receiver<()>,
) {
    let state = Arc::new((Mutex::new(Capture::default()), Condvar::new()));
    let (tx, rx) = mpsc::channel();
    let output = TerminalOutput::with_writer(SlowWriter {
        state: state.clone(),
        entered: Some(tx),
        fail_flush,
    })
    .unwrap();
    (output, state, rx)
}
fn release(state: &Arc<(Mutex<Capture>, Condvar)>) {
    state.0.lock().unwrap().open = true;
    state.1.notify_all();
}
fn encoded(encoder: &mut BlitEncoder, frame: FrameData, repaint: bool) -> Vec<u8> {
    let encoded = encoder.encode(&frame, repaint);
    let bytes = encoded.bytes.clone();
    encoder.commit(frame, encoded);
    bytes
}

#[test]
fn blocked_writer_coalesces_semantic_frames_against_the_written_baseline() {
    let (mut output, state, entered) = slow(false);
    let first = frame("A", 4);
    let final_frame = frame("Z", 4);
    output.frame(first.clone(), false, false).unwrap();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        output.handle.snapshot().2,
        0,
        "blocked bytes are not a baseline"
    );
    for _ in 0..1000 {
        output.frame(frame("B", 4), false, false).unwrap();
    }
    output.frame(final_frame.clone(), false, false).unwrap();
    let (pending, bytes, _) = output.handle.snapshot();
    assert_eq!(pending, 1);
    assert!(
        bytes < 4096,
        "memory is bounded independently of update count"
    );
    release(&state);
    output.finish().unwrap();
    let mut encoder = BlitEncoder::new();
    let mut expected = encoded(&mut encoder, first, false);
    expected.extend(encoded(&mut encoder, final_frame, false));
    assert_eq!(state.0.lock().unwrap().bytes, expected);
    assert_eq!(output.handle.snapshot().2, 1002);
}

#[test]
fn coalesced_patch_keeps_all_semantic_changes_and_the_latest_cursor() {
    let (mut output, state, entered) = slow(false);
    output.frame(frame("A", 4), false, false).unwrap();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let mut latest = frame("A", 4);
    for (x, symbol) in [(0, "X"), (3, "Y")] {
        let cell = frame(symbol, 1).cells.remove(0);
        output
            .patch(
                &[PaneSurfacePatchRow {
                    x,
                    y: 0,
                    cells: vec![cell.clone()],
                }],
                None,
                false,
            )
            .unwrap();
        latest.cells[x as usize] = cell;
    }
    let cursor = CursorState {
        x: 2,
        y: 0,
        visible: true,
        shape: 6,
    };
    output.patch(&[], Some(cursor.clone()), false).unwrap();
    latest.cursor = Some(cursor);
    release(&state);
    output.finish().unwrap();
    let mut encoder = BlitEncoder::new();
    let mut expected = encoded(&mut encoder, frame("A", 4), false);
    expected.extend(encoded(&mut encoder, latest, false));
    assert_eq!(state.0.lock().unwrap().bytes, expected);
}

#[test]
fn graphics_and_effect_barriers_preserve_order_and_complete_partial_sequences() {
    let (mut output, state, entered) = slow(false);
    output.frame(frame("A", 2), false, false).unwrap();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let mut image = frame("B", 2);
    image.graphics = b"\x1b_Ga=p,i=17;\x1b\\".to_vec();
    output.frame(image.clone(), false, false).unwrap();
    {
        let _route = output.route();
        let mut title = stdout();
        write!(title, "\x1b]0;{}", "queued").unwrap();
        title.flush().unwrap();
    }
    output.graphics(b"\x1b_Ga=d,i=17;\x1b\\").unwrap();
    output.frame(frame("C", 2), false, false).unwrap();
    release(&state);
    output.finish().unwrap();
    let mut encoder = BlitEncoder::new();
    let mut expected = encoded(&mut encoder, frame("A", 2), false);
    let encoded_image = encoder.encode(&image, false);
    super::super::frame_output::write_encoded_frame_with_graphics(
        &mut expected,
        &encoded_image.bytes,
        &image.graphics,
    )
    .unwrap();
    encoder.commit(image, encoded_image);
    expected.extend_from_slice(b"\x1b]0;queued\x07\x1b7\x1b_Ga=d,i=17;\x1b\\\x1b8");
    expected.extend(encoded(&mut encoder, frame("C", 2), false));
    assert_eq!(state.0.lock().unwrap().bytes, expected);
}

#[test]
fn resize_roundtrip_retains_invalidation_when_text_frames_coalesce() {
    let (mut output, state, entered) = slow(false);
    output.frame(frame("A", 4), false, false).unwrap();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    output.invalidate();
    output.frame(frame("B", 4), true, false).unwrap();
    output.frame(frame("C", 4), false, false).unwrap();
    release(&state);
    output.finish().unwrap();
    let mut encoder = BlitEncoder::new();
    let mut expected = encoded(&mut encoder, frame("A", 4), false);
    encoder.invalidate();
    expected.extend(encoded(&mut encoder, frame("C", 4), true));
    assert_eq!(state.0.lock().unwrap().bytes, expected);
}

#[test]
fn failed_flush_never_acknowledges_or_commits_a_frame() {
    let (mut output, state, entered) = slow(true);
    output.frame(frame("A", 4), false, false).unwrap();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    release(&state);
    assert!(output.finish().is_err());
    assert_eq!(output.handle.snapshot().2, 0);
    assert!(output.feedback().is_err());
    assert!(output.frame(frame("B", 4), false, false).is_err());
}

#[test]
fn lossless_effect_overflow_fails_explicitly_without_interrupting_active_bytes() {
    let (mut output, state, entered) = slow(false);
    output.frame(frame("A", 4), false, false).unwrap();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    for _ in 0..256 {
        output.handle.push(Command::Bytes(vec![b'!'])).unwrap();
    }
    assert!(output.handle.push(Command::Bytes(vec![b'?'])).is_err());
    release(&state);
    assert!(output.finish().is_err());
    let mut encoder = BlitEncoder::new();
    let mut expected = encoded(&mut encoder, frame("A", 4), false);
    expected.extend_from_slice(&[b'!'; 256]);
    assert_eq!(state.0.lock().unwrap().bytes, expected);
}

#[test]
fn composition_defers_until_output_drains_and_then_uses_the_current_model() {
    let (mut output, state, entered) = slow(false);
    output.frame(frame("A", 4), false, false).unwrap();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let _route = output.route();
    let mut shell = crate::client::compositor::ClientShellState::new(
        crate::client::compositor::ClientShellConfig::from_config(
            &crate::utils::config::Config::default(),
        ),
    );
    assert!(shell.compose(80, 24).is_none());
    assert!(shell.compose_deferred);
    release(&state);
    output.finish().unwrap();
    assert!(shell.compose(80, 24).is_some());
    assert!(!shell.compose_deferred);
}

#[test]
fn written_patch_tracks_hardware_and_drawn_cursor_baselines() {
    for drawn in [false, true] {
        let (mut output, state, entered) = slow(false);
        let mut first = frame("A", 4);
        first.cursor = Some(CursorState {
            x: 0,
            y: 0,
            visible: true,
            shape: 2,
        });
        output.frame(first.clone(), false, drawn).unwrap();
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        release(&state);
        output.handle.wait_idle();
        let cursor = CursorState {
            x: 2,
            y: 0,
            visible: true,
            shape: 6,
        };
        let rows = vec![PaneSurfacePatchRow {
            x: 1,
            y: 0,
            cells: frame("B", 1).cells,
        }];
        assert!(output.patch(&rows, Some(cursor.clone()), drawn).unwrap());
        output.handle.wait_idle();
        let mut last = first.clone();
        last.cells[1] = rows[0].cells[0].clone();
        last.cursor = Some(cursor.clone());
        last.cells[3].symbol = "C".into();
        output.frame(last.clone(), false, drawn).unwrap();
        output.finish().unwrap();
        let mut encoder = BlitEncoder::new();
        let first = if drawn {
            frame_with_drawn_cursor(first)
        } else {
            first
        };
        let initial = if drawn {
            encoder.encode_with_suppressed_visible_cursor(&first, false)
        } else {
            encoder.encode(&first, false)
        };
        let mut expected = initial.bytes.clone();
        encoder.commit(first, initial);
        let rows = if drawn {
            encoder
                .patch_rows_with_drawn_cursor(&rows, Some(&cursor))
                .unwrap()
        } else {
            rows
        };
        let patch = encoder
            .encode_patch(&rows, Some(cursor.clone()), drawn)
            .unwrap();
        expected.extend_from_slice(&patch.bytes);
        assert!(encoder.commit_patch(&rows, Some(cursor), patch));
        let last = if drawn {
            frame_with_drawn_cursor(last)
        } else {
            last
        };
        let full = if drawn {
            encoder.encode_with_suppressed_visible_cursor(&last, false)
        } else {
            encoder.encode(&last, false)
        };
        expected.extend_from_slice(&full.bytes);
        assert_eq!(state.0.lock().unwrap().bytes, expected);
    }
}

#[test]
fn ansi_expansion_is_bounded_before_encoding_large_repeated_hyperlinks() {
    let (mut output, state, _) = slow(false);
    let mut request = frame("A", 64);
    request
        .hyperlinks
        .push("https://example.test/".to_owned() + &"x".repeat(1024 * 1024));
    for cell in &mut request.cells {
        cell.hyperlink = Some(0);
    }
    assert!(output.frame(request, false, false).is_err());
    assert!(output.finish().is_err());
    assert!(state.0.lock().unwrap().bytes.is_empty());
}

#[test]
fn shutdown_and_terminal_restore_wait_for_the_whole_active_transaction() {
    let (mut output, state, entered) = slow(false);
    output.frame(frame("A", 4), false, false).unwrap();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let (closing_tx, closing_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let closer = std::thread::spawn(move || {
        let _route = output.route();
        closing_tx.send(()).unwrap();
        let _exclusive_restore = suspend();
        assert!(!busy());
        output.finish().unwrap();
        done_tx.send(()).unwrap();
    });
    closing_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        done_rx.recv_timeout(Duration::from_millis(50)).is_err(),
        "restoration must wait for the blocked ANSI transaction"
    );
    release(&state);
    done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    closer.join().unwrap();
    assert_eq!(
        state.0.lock().unwrap().bytes,
        encoded(&mut BlitEncoder::new(), frame("A", 4), false)
    );
}
