#[test]
fn protocol_from_zero_flags_is_legacy() {
    assert_eq!(
        KeyboardProtocol::from_kitty_flags(0),
        KeyboardProtocol::Legacy
    );
}

#[test]
fn protocol_from_nonzero_flags_is_kitty() {
    assert_eq!(
        KeyboardProtocol::from_kitty_flags(7),
        KeyboardProtocol::Kitty { flags: 7 }
    );
}
