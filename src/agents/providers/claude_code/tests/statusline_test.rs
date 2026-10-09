use super::*;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "bus-statusline-{}-{}",
            std::process::id(),
            bus_io::now_ns()
        ));
        std::fs::create_dir_all(&path).unwrap();
        atomic_write(
            &path.join("manifest.json"),
            &serde_json::to_vec(&ObservationManifest {
                agent_id: 7,
                provider: ProviderKind::ClaudeCode,
                launch_id: "launch".into(),
            })
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            path.join("claude-settings.json"),
            br#"{"hooks":{"Stop":[]}}"#,
        )
        .unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn parses_allowance_not_context_and_keeps_missing_or_invalid_windows_unknown() {
    let parsed = windows(
        &json!({"context_window":{"used_percentage":99},"rate_limits":{
        "five_hour":{"used_percentage":12.5,"resets_at":1791788174},
        "seven_day":{"used_percentage":35,"resets_at":1791888174}}}),
    );
    assert_eq!(
        parsed.five_hour,
        Some(UsageWindow {
            used_percent: 12.5,
            resets_at: Some(1791788174),
            window_minutes: 300
        })
    );
    assert_eq!(parsed.weekly.unwrap().used_percent, 35.0);
    assert_eq!(windows(&json!({})), UsageWindows::default());
    for invalid in [json!(-1), json!(101), json!("12"), Value::Null] {
        assert_eq!(
            windows(
                &json!({"rate_limits":{"five_hour":{"used_percentage":invalid},"seven_day":null}})
            ),
            UsageWindows::default()
        );
    }
    assert_eq!(
        windows(&json!({"rate_limits":{"five_hour":{"used_percentage":0}}}))
            .five_hour
            .unwrap()
            .used_percent,
        0.0
    );
}

#[test]
fn settings_follow_scope_precedence_and_preserve_hooks_and_presentation() {
    let fixture = Fixture::new();
    let user = fixture.0.join("user.json");
    let project = fixture.0.join("project.json");
    let local = fixture.0.join("local.json");
    std::fs::write(
        &user,
        br#"{"statusLine":{"type":"command","command":"user-hud","padding":2}}"#,
    )
    .unwrap();
    std::fs::write(
        &project,
        br#"{"statusLine":{"command":"project-hud","refreshInterval":5}}"#,
    )
    .unwrap();
    std::fs::write(&local, br#"{"statusLine":{"command":"local-hud"}}"#).unwrap();
    let paths = [user.clone(), project.clone(), local.clone()];
    let before: Vec<_> = paths.iter().map(|p| std::fs::read(p).unwrap()).collect();
    let original = original_settings(&paths);
    assert_eq!(original["command"], "local-hud");
    install_with_settings(&fixture.0, Path::new("/tmp/a bus"), original.clone()).unwrap();
    assert_eq!(read_json(&fixture.0.join(ORIGINAL)).unwrap(), original);
    let settings = read_json(&fixture.0.join("claude-settings.json")).unwrap();
    assert_eq!(settings["hooks"], json!({"Stop":[]}));
    #[cfg(unix)]
    assert_eq!(
        settings["statusLine"]["command"],
        "'/tmp/a bus' --bus-callback claude-statusline"
    );
    assert_eq!(settings["statusLine"]["padding"], 2);
    assert_eq!(settings["statusLine"]["refreshInterval"], 5);
    assert_eq!(
        paths
            .iter()
            .map(|p| std::fs::read(p).unwrap())
            .collect::<Vec<_>>(),
        before
    );
    std::fs::write(&local, br#"{"statusLine":null}"#).unwrap();
    assert_eq!(original_settings(&paths), Value::Null);
}

#[cfg(unix)]
#[test]
fn passthrough_keeps_exact_stdin_and_ansi_output_even_on_capture_or_command_failure() {
    let fixture = Fixture::new();
    install_with_settings(
        &fixture.0,
        Path::new("/tmp/bus"),
        json!({"type":"command","command":r"printf '\033[32m'; cat; printf '\033[0m\n\n'; exit 7"}),
    )
    .unwrap();
    for input in [
        b"not json\n".as_slice(),
        br#"{"session_id":"s","rate_limits":{"five_hour":{"used_percentage":3}}}"#,
    ] {
        let mut expected = b"\x1b[32m".to_vec();
        expected.extend_from_slice(input);
        expected.extend_from_slice(b"\x1b[0m\n\n");
        assert_eq!(
            render(Some(&fixture.0), "wrong-launch", input, COMMAND_TIMEOUT),
            expected
        );
        assert!(!fixture.0.join(USAGE).exists());
    }
}

#[cfg(unix)]
#[test]
fn hung_commands_and_nonreading_stdin_are_bounded() {
    let start = std::time::Instant::now();
    let output = passthrough(
        "printf ready; sleep 10",
        &vec![b'x'; MAX_BYTES as usize],
        Duration::from_millis(100),
    )
    .unwrap();
    assert_eq!(output, b"ready");
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(
        passthrough("command-that-does-not-exist-bus", b"{}", COMMAND_TIMEOUT)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn capture_is_launch_bound_bounded_and_missing_data_replaces_old_windows() {
    let fixture = Fixture::new();
    let first = br#"{"session_id":"s","rate_limits":{"five_hour":{"used_percentage":3}}}"#;
    capture(&fixture.0, "launch", first).unwrap();
    let saved = read_usage(&fixture.0).unwrap();
    assert_eq!(saved.manifest.agent_id, 7);
    assert_eq!(saved.session_id, "s");
    assert_eq!(saved.windows.five_hour.unwrap().used_percent, 3.0);
    assert!(capture(&fixture.0, "wrong", first).is_err());
    capture(&fixture.0, "launch", br#"{"session_id":"s"}"#).unwrap();
    assert_eq!(
        read_usage(&fixture.0).unwrap().windows,
        UsageWindows::default()
    );
    assert_eq!(
        std::fs::read_dir(&fixture.0)
            .unwrap()
            .filter(|p| p.as_ref().unwrap().file_name() == USAGE)
            .count(),
        1
    );
    assert!(render(None, "", b"{}", COMMAND_TIMEOUT).is_empty());
    install_with_settings(&fixture.0, Path::new("/tmp/bus"), json!({})).unwrap();
    assert_eq!(
        render(Some(&fixture.0), "launch", b"{}", COMMAND_TIMEOUT),
        b"Claude | Bus\n"
    );
}
