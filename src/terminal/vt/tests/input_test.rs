use super::callbacks::MAX_CLIPBOARD_BYTES;
use super::*;

fn test_clipboard_content(mime: &[u8], data: &[u8]) -> ffi::GhosttyClipboardContent {
    ffi::GhosttyClipboardContent {
        mime: ffi::GhosttyString {
            ptr: mime.as_ptr(),
            len: mime.len(),
        },
        data: ffi::GhosttyString {
            ptr: data.as_ptr(),
            len: data.len(),
        },
    }
}

fn invoke_clipboard_callback(
    terminal: &mut Terminal,
    contents: &[ffi::GhosttyClipboardContent],
) -> ffi::GhosttyClipboardWriteResult {
    let request = ffi::GhosttyClipboardWrite {
        size: std::mem::size_of::<ffi::GhosttyClipboardWrite>(),
        location: ffi::GhosttyClipboardLocation_GHOSTTY_CLIPBOARD_LOCATION_STANDARD,
        contents: contents.as_ptr(),
        contents_len: contents.len(),
    };
    // SAFETY: the request and its borrowed content live through this call.
    unsafe {
        clipboard_write_trampoline(
            terminal.raw,
            (&mut *terminal.callback_state as *mut TerminalCallbackState).cast(),
            &request,
        )
    }
}

#[test]
fn focus_encoding_matches_expected_sequences() {
    assert_eq!(encode_focus(FocusEvent::Gained).unwrap(), b"\x1b[I");
    assert_eq!(encode_focus(FocusEvent::Lost).unwrap(), b"\x1b[O");
}

#[test]
fn terminal_callbacks_report_pty_responses_and_pwd_changes() {
    let mut terminal = Terminal::new(8, 3, 100).unwrap();
    let responses = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let sink = responses.clone();
    terminal
        .set_write_pty_callback(move |bytes| sink.lock().unwrap().extend_from_slice(bytes))
        .unwrap();

    terminal.write(b"\x1b[6n\x1b]7;file:///tmp/herdr\x07");

    let output = responses.lock().unwrap().clone();
    assert!(!output.is_empty());
    assert!(String::from_utf8_lossy(&output).contains("R"));
    assert_eq!(terminal.take_pwd_changes(), [b"file:///tmp/herdr".to_vec()]);
}

#[test]
fn key_and_mouse_encoders_follow_terminal_state() {
    let mut terminal = Terminal::new(80, 24, 0).unwrap();
    terminal.mode_set(1, true).unwrap();
    terminal.write(b"\x1b[>1u\x1b[?1000h\x1b[?1006h");

    assert!(terminal.mode_get(1).unwrap());
    assert_eq!(terminal.kitty_keyboard_flags().unwrap(), 1);
    assert!(terminal.mouse_tracking_enabled().unwrap());

    let mut key_encoder = KeyEncoder::new().unwrap();
    key_encoder.set_from_terminal(&terminal);
    let mut key_event = KeyEvent::new().unwrap();
    key_event.set_action(ffi::GhosttyKeyAction_GHOSTTY_KEY_ACTION_PRESS);
    key_event.set_key(ffi::GhosttyKey_GHOSTTY_KEY_A);
    key_event.set_mods(MOD_CTRL | MOD_SHIFT);
    key_event.set_utf8("A");
    key_event.set_unshifted_codepoint('a' as u32);
    let encoded_key = key_encoder.encode(&key_event).unwrap();
    assert_eq!(encoded_key, b"\x1b[97;6u");

    let mut mouse_encoder = MouseEncoder::new().unwrap();
    mouse_encoder.set_from_terminal(&terminal);
    mouse_encoder.set_size(80, 24, 1, 1);
    let mut mouse_event = MouseEvent::new().unwrap();
    mouse_event.set_action(ffi::GhosttyMouseAction_GHOSTTY_MOUSE_ACTION_PRESS);
    mouse_event.set_button(ffi::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_LEFT);
    mouse_event.set_position(0.0, 0.0);
    let encoded_mouse = mouse_encoder.encode(&mouse_event).unwrap();
    assert_eq!(encoded_mouse, b"\x1b[<0;1;1M");
}

#[test]
fn clipboard_callback_ignores_clear_and_rejects_unsupported_writes() {
    let mut terminal = Terminal::new(10, 5, 0).unwrap();
    let success = ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_SUCCESS;
    let unsupported = ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_UNSUPPORTED;
    let invalid = ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_INVALID_DATA;

    assert_eq!(invoke_clipboard_callback(&mut terminal, &[]), success);
    assert!(terminal.take_clipboard_writes().is_empty());

    let empty = test_clipboard_content(b"text/plain", b"");
    assert_eq!(
        invoke_clipboard_callback(&mut terminal, &[empty]),
        unsupported
    );
    let text = test_clipboard_content(b"text/plain", b"text");
    let image = test_clipboard_content(b"image/png", b"image");
    assert_eq!(
        invoke_clipboard_callback(&mut terminal, &[text, image]),
        unsupported
    );

    let oversized = vec![b'x'; MAX_CLIPBOARD_BYTES + 1];
    let oversized = test_clipboard_content(b"text/plain", &oversized);
    assert_eq!(
        invoke_clipboard_callback(&mut terminal, &[oversized]),
        invalid
    );
    assert!(terminal.take_clipboard_writes().is_empty());
}

#[test]
fn libghostty_completes_osc52_writes_for_bel_and_st_without_queries() {
    let mut terminal = Terminal::new(10, 5, 0).unwrap();
    terminal.write(b"\x1b]52;c;aGVs");
    assert!(terminal.take_clipboard_writes().is_empty());
    terminal.write(b"bG8=\x07");
    assert_eq!(terminal.take_clipboard_writes(), vec![b"hello".to_vec()]);

    terminal.write(b"\x1b]52;c;d29ybGQ=\x1b\\");
    assert_eq!(terminal.take_clipboard_writes(), vec![b"world".to_vec()]);

    terminal.write(b"\x1b]52;c;?\x07");
    assert!(terminal.take_clipboard_writes().is_empty());

    terminal.write(b"\x1b]52;c;\x07");
    assert!(terminal.take_clipboard_writes().is_empty());
}
