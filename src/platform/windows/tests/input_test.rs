#[test]
fn paste_text_uses_windows_line_endings() {
    assert_eq!(
        super::super::prepare_paste_text_for_pty_platform("one\ntwo\r\nthree\rfour".to_owned()),
        "one\r\ntwo\r\nthree\rfour"
    );
}
