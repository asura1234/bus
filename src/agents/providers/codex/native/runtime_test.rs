use super::*;

#[test]
fn codex_native_attached_profile_retains_the_configured_tui_without_starting_a_server() {
    let dir = crate::utils::test_temp::unique_temp_path("codex-profile");
    fs::create_dir_all(&dir).unwrap();
    let mut runtime = Runtime {
        config: Some(Config {
            capture: dir.clone(),
        }),
        session: None,
    };
    let argv = ["/not-a-program/codex", "-pwork", "--no-daemon"].map(str::to_owned);
    let result = runtime.launch(&argv).unwrap();
    let started = dir.join("codex-native.log").exists();
    fs::remove_dir_all(dir).unwrap();
    assert_eq!(result, argv);
    assert!(!started,"a config profile must select the legacy TUI before a differently configured app-server starts");
}

#[test]
fn codex_native_explicit_permission_cli_retains_the_configured_tui_before_a_backend_starts() {
    use std::os::unix::fs::PermissionsExt;
    let dir = crate::utils::test_temp::unique_temp_path("codex-policy-fallback");
    fs::create_dir_all(&dir).unwrap();
    let program = dir.join("codex");
    let marker = dir.join("backend-started");
    fs::write(
        &program,
        format!(
            "#!/bin/sh\nprintf started > '{}'\nexit 1\n",
            marker.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
    let mut runtime = Runtime {
        config: Some(Config {
            capture: dir.clone(),
        }),
        session: None,
    };
    let mut attempted = Vec::new();
    for flags in [
        vec!["--sandbox", "read-only", "--ask-for-approval", "on-request"],
        vec!["--sandbox=read-only"],
        vec!["--ask-for-approval=on-request"],
        vec!["-sread-only"],
        vec!["-aon-request"],
        vec!["--yolo"],
        vec!["--dangerously-bypass-approvals-and-sandbox"],
    ] {
        let mut argv = vec![program.to_string_lossy().into_owned(), "--no-daemon".into()];
        argv.extend(flags.into_iter().map(str::to_owned));
        argv.extend(["resume".into(), "fixture-thread".into()]);
        assert_eq!(runtime.launch(&argv).unwrap(), argv);
        if marker.exists() {
            attempted.push(argv);
            fs::remove_file(&marker).unwrap();
        }
    }
    drop(runtime);
    fs::remove_dir_all(dir).unwrap();
    assert!(attempted.is_empty(), "incompatible remote permission options must choose the original TUI before starting a native writer: {attempted:?}");
}

#[test]
fn codex_native_fresh_permission_options_select_the_available_native_transport() {
    // Real Codex0.164.0 thread/start witnesses establish both policy contracts.
    for args in [
        vec!["--sandbox", "read-only", "--ask-for-approval", "on-request"],
        vec!["--yolo"],
    ] {
        let args = args.into_iter().map(str::to_owned).collect::<Vec<_>>();
        assert!(
            supports_native(&args),
            "fresh permissions support native thread/start: {args:?}"
        );
    }
}

#[test]
fn codex_native_permission_fallback_uses_effective_resume_or_fork_options() {
    for (args, native) in [
        (vec!["--sandbox=read-only", "resume", "thread"], false),
        (vec!["fork", "thread", "--yolo"], false),
        (vec!["--yolo", "-c", "resume"], true),
        (vec!["resume", "thread", "--", "--sandbox=read-only"], true),
        (
            vec!["-c", "note=--sandbox=read-only", "resume", "thread"],
            true,
        ),
    ] {
        let args = args.into_iter().map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(supports_native(&args), native, "{args:?}");
    }
}

#[test]
fn codex_runtime_preparations_at_the_same_clock_tick_have_unique_endpoints() {
    let first = Session::prepare_at(123).unwrap();
    let second = Session::prepare_at(123).expect("a clock tick is not a unique launch identity");
    let first_dir = first.directory.clone();
    let second_dir = second.directory.clone();
    assert_ne!(first_dir, second_dir);
    drop(first);
    drop(second);
    assert!(!first_dir.exists());
    assert!(!second_dir.exists());
}
