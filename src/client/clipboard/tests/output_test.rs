use super::*;

#[test]
fn osc52_sequence_uses_bel_terminator() {
    assert_eq!(osc52_sequence(b"hello"), "\x1b]52;c;aGVsbG8=\x07");
}

#[test]
fn ssh_sessions_prefer_osc52() {
    assert!(should_prefer_osc52_for_env(
        Some(OsStr::new("1 2 3 4")),
        None,
        None,
        false
    ));
    assert!(should_prefer_osc52_for_env(
        None,
        Some(OsStr::new("/dev/ttys001")),
        None,
        false
    ));
    assert!(!should_prefer_osc52_for_env(None, None, None, false));
}

#[test]
fn wsl_sessions_prefer_osc52() {
    assert!(should_prefer_osc52_for_env(None, None, None, true));
}

#[test]
fn vscode_remote_sessions_prefer_osc52() {
    assert!(should_prefer_osc52_for_env(
        None,
        None,
        Some(OsStr::new("/tmp/vscode-remote-cli.sock")),
        false
    ));
}

#[test]
fn wsl_detection_uses_env_vars() {
    assert!(is_wsl_for_env(
        None,
        None,
        Some(OsStr::new("Ubuntu")),
        None,
        false
    ));
    assert!(is_wsl_for_env(
        None,
        None,
        None,
        Some(OsStr::new("/run/WSL/123_interop")),
        false
    ));
}

#[test]
fn wsl_detection_uses_kernel_markers() {
    assert!(is_wsl_for_env(
        Some("5.15.167.4-microsoft-standard-WSL2"),
        None,
        None,
        None,
        false
    ));
    assert!(is_wsl_for_env(
        None,
        Some("Linux version 5.15.167.4-microsoft-standard-WSL2"),
        None,
        None,
        false
    ));
}

#[test]
fn wsl_detection_ignores_non_wsl_kernel_strings() {
    assert!(!contains_wsl_marker("notwsl-kernel"));
    assert!(!is_wsl_for_env(
        Some("6.8.0-31-generic"),
        Some("Linux version 6.8.0-31-generic"),
        None,
        None,
        false
    ));
}

#[test]
fn wsl_detection_uses_wsl_runtime_markers() {
    assert!(is_wsl_for_env(None, None, None, None, true));
    assert!(!is_wsl_for_env(None, None, None, None, false));
}
