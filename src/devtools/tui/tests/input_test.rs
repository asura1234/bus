use super::*;
use crate::protocol::keys::KeyboardProtocol;

#[test]
fn chords_use_the_negotiated_protocol() {
    assert_eq!(
        encode_key("enter", KeyboardProtocol::Legacy).unwrap(),
        b"\r"
    );
    assert_eq!(
        encode_key("ctrl+n", KeyboardProtocol::Legacy).unwrap(),
        b"\x0e"
    );
    // With kitty disambiguation Esc is CSI 27 u, not a lone ESC.
    let kitty = KeyboardProtocol::from_kitty_flags(1);
    assert_eq!(encode_key("esc", kitty).unwrap(), b"\x1b[27u");
    assert!(encode_key("+", KeyboardProtocol::Legacy).is_ok());
    assert!(encode_key("nonsense-key", KeyboardProtocol::Legacy).is_err());
}

#[test]
fn clicks_are_sgr_press_and_release_at_one_based_cells() {
    assert_eq!(
        click(4, 9, Button::Left, Mods::default()),
        b"\x1b[<0;10;5M\x1b[<0;10;5m"
    );
    let mods = Mods::parse("shift,ctrl").unwrap();
    assert_eq!(
        click(0, 0, Button::Right, mods),
        b"\x1b[<22;1;1M\x1b[<22;1;1m"
    );
    assert!(Mods::parse("hyper").is_err());
    assert_eq!(Button::parse("middle").unwrap(), Button::Middle);
    assert!(Button::parse("x").is_err());
}

#[test]
fn drags_press_move_and_release() {
    let steps = drag((1, 1), (1, 3), Button::Left, Mods::default());
    assert_eq!(steps.first().unwrap(), b"\x1b[<0;2;2M");
    assert_eq!(steps[1], b"\x1b[<32;3;2M");
    assert_eq!(steps.last().unwrap(), b"\x1b[<0;4;2m");
    assert_eq!(steps.len(), 4);
}

#[test]
fn wheel_and_paste_encodings() {
    assert_eq!(
        scroll(2, 3, true, 2, Mods::default()),
        b"\x1b[<64;4;3M\x1b[<64;4;3M"
    );
    assert_eq!(scroll(0, 0, false, 1, Mods::default()), b"\x1b[<65;1;1M");
    assert_eq!(paste("a", true), b"\x1b[200~a\x1b[201~");
    assert_eq!(paste("a", false), b"a");
}
