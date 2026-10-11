use super::*;

#[test]
fn replays_drop_clipboard_notifications_and_bells_but_keep_graphics() {
    let input = b"a\x07b\x1b]52;c;aGk=\x07c\x1b]9;done\x1b\\d\x1b]99;;x\x07e\x1b]0;title\x07\x1b_Gf=100;AAAA\x1b\\f\x1b]777;notify;t;b\x07";
    assert_eq!(
        strip_effects(input),
        b"abcde\x1b]0;title\x07\x1b_Gf=100;AAAA\x1b\\f".to_vec()
    );
}

#[test]
fn an_unterminated_string_is_kept_whole() {
    assert_eq!(strip_effects(b"x\x1b]0;open"), b"x\x1b]0;open".to_vec());
}
