use super::encode;

fn notification(program: &str, term: &str, kitty: bool, tmux: bool) -> Vec<u8> {
    static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _serial = ENV.lock().unwrap();
    let values = [
        ("TERM_PROGRAM", Some(program)),
        ("TERM", Some(term)),
        ("KITTY_WINDOW_ID", kitty.then_some("fixture")),
        ("TMUX", tmux.then_some("fixture")),
    ];
    let previous: Vec<_> = values
        .iter()
        .map(|(key, _)| (*key, std::env::var_os(key)))
        .collect();
    for (key, value) in values {
        if let Some(value) = value {
            std::env::set_var(key, value);
        } else {
            std::env::remove_var(key);
        }
    }
    let bytes = encode("Bus froze for 3.5 s at 09:07; details: bus perf");
    for (key, value) in previous {
        if let Some(value) = value {
            std::env::set_var(key, value);
        } else {
            std::env::remove_var(key);
        }
    }
    bytes
}

#[test]
fn ghostty_recovery_uses_osc9() {
    assert_eq!(
        notification("ghostty", "xterm-256color", false, false),
        b"\x1b]9;Bus froze for 3.5 s at 09:07; details: bus perf\x1b\\"
    );
}

#[test]
fn tmux_recovery_escapes_and_wraps_the_complete_notification() {
    assert_eq!(
        notification("ghostty", "screen-256color", false, true),
        b"\x1bPtmux;\x1b\x1b]9;Bus froze for 3.5 s at 09:07; details: bus perf\x1b\x1b\\\x1b\\"
    );
}

#[test]
fn kitty_recovery_uses_its_notification_protocol() {
    assert_eq!(
        notification("", "xterm-kitty", true, false),
        b"\x1b]99;;Bus froze for 3.5 s at 09:07; details: bus perf\x1b\\"
    );
}

#[test]
fn unidentified_terminal_keeps_the_recovery_osc9_default() {
    assert_eq!(
        notification("", "xterm-256color", false, false),
        b"\x1b]9;Bus froze for 3.5 s at 09:07; details: bus perf\x1b\\"
    );
}
