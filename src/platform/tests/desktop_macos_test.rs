#[test]
fn terminal_bundle_identifier_maps_known_terminal_env() {
    assert_eq!(
        terminal_bundle_identifier_from_env(Some("ghostty"), None, false, false),
        Some("com.mitchellh.ghostty")
    );
    assert_eq!(
        terminal_bundle_identifier_from_env(Some("iTerm.app"), None, false, false),
        Some("com.googlecode.iterm2")
    );
    assert_eq!(
        terminal_bundle_identifier_from_env(Some("WezTerm"), None, false, false),
        Some("com.github.wez.wezterm")
    );
    assert_eq!(
        terminal_bundle_identifier_from_env(Some("Apple_Terminal"), None, false, false),
        Some("com.apple.Terminal")
    );
    assert_eq!(
        terminal_bundle_identifier_from_env(None, Some("xterm-kitty"), false, false),
        Some("net.kovidgoyal.kitty")
    );
    assert_eq!(
        terminal_bundle_identifier_from_env(None, None, true, false),
        Some("net.kovidgoyal.kitty")
    );
    assert_eq!(
        terminal_bundle_identifier_from_env(None, None, false, true),
        Some("org.alacritty")
    );
    assert_eq!(
        terminal_bundle_identifier_from_env(None, None, false, false),
        None
    );
}

#[test]
fn terminal_notifier_command_includes_icon_and_activation() {
    let mut cmd = Command::new("terminal-notifier");
    build_terminal_notifier_command(
        &mut cmd,
        "pi finished",
        Some("workspace 1"),
        Some("com.mitchellh.ghostty"),
    );
    let args = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        args,
        vec![
            "-title",
            "pi finished",
            "-message",
            "workspace 1",
            "-activate",
            "com.mitchellh.ghostty"
        ]
    );
}

#[test]
fn terminal_notifier_success_skips_osascript() {
    let path =
        std::env::temp_dir().join(format!("bus-terminal-notifier-args-{}", std::process::id()));
    let script = "printf '%s:%s\\n' \"$0\" \"$*\" >> \"$BUS_NOTIFY_ARGS\"";
    let mut command = |program: &str| {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(script)
            .arg(program)
            .env("BUS_NOTIFY_ARGS", &path);
        cmd
    };

    let shown = show_terminal_notifier_notification_with_options(
        "title",
        Some("body"),
        Some("com.mitchellh.ghostty"),
        &mut command,
    )
    .expect("terminal-notifier command should run");

    assert!(shown);
    let args = std::fs::read_to_string(&path).expect("args file");
    let _ = std::fs::remove_file(&path);
    assert!(args.starts_with("terminal-notifier:"), "{args}");
    assert!(args.contains("-activate com.mitchellh.ghostty"), "{args}");
    assert!(!args.contains("osascript"), "{args}");
}

#[test]
fn desktop_notification_falls_back_to_osascript_when_terminal_notifier_fails() {
    let path = std::env::temp_dir().join(format!("bus-osascript-args-{}", std::process::id()));
    let script = r#"
if [ "$0" = "terminal-notifier" ]; then
  exit 1
fi
printf '%s\n' "$@" > "$BUS_NOTIFY_ARGS"
"#;
    let mut command = |program: &str| {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(script)
            .arg(program)
            .env("BUS_NOTIFY_ARGS", &path);
        cmd
    };
    let shown = show_desktop_notification_with_command("title", Some("body"), &mut command)
        .expect("osascript fallback should run");

    assert!(shown);
    let args = std::fs::read_to_string(&path).expect("args file");
    let _ = std::fs::remove_file(&path);
    assert_eq!(
            args,
            "-e\non run argv\n-e\ndisplay notification (item 2 of argv) with title (item 1 of argv)\n-e\nend run\ntitle\nbody\n"
        );
}

#[test]
fn scrollback_editor_argv_preserves_unix_editor_shell_semantics() {
    let path = std::path::Path::new("/tmp/bus scrollback.txt");
    let argv = scrollback_editor_argv(path).unwrap();

    assert_eq!(argv[0], "/bin/sh");
    assert_eq!(argv[1], "-c");
    assert!(argv[2].contains("EDITOR:-vi"));
    assert!(argv[2].contains("/tmp/bus scrollback.txt"));
}
