use super::*;

fn stop(child: &mut Child) {
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.wait();
}

#[test]
fn codex_native_server_exits_when_its_bus_lifetime_pipe_closes() {
    let dir = crate::utils::test_temp::unique_temp_path("codex-lifetime");
    fs::create_dir_all(&dir).unwrap();
    let lock = acquire_launch_lock(&dir.join("owner")).unwrap();
    let mut child = owned_command(&["/bin/sleep".into(), "60".into()], &lock)
        .process_group(0)
        .spawn()
        .unwrap();
    let pipe = child.stdin.take();
    let has_pipe = pipe.is_some();
    drop(pipe);
    let deadline = Instant::now() + Duration::from_secs(3);
    while matches!(child.try_wait(), Ok(None)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let ended = matches!(child.try_wait(), Ok(Some(_)));
    if !ended {
        stop(&mut child);
    }
    assert!(
        has_pipe && ended,
        "a dead Bus must not leave a writer runtime running"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn codex_native_server_holds_the_writer_lock_after_its_bus_handle_drops() {
    if std::env::var_os("BUS_CODEX_LOCK_PROBE").is_none() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "agents::providers::codex::native::runtime::tests::codex_native_server_holds_the_writer_lock_after_its_bus_handle_drops", "--test-threads=1"])
            .env("BUS_CODEX_LOCK_PROBE", "1")
            .output().unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let dir = crate::utils::test_temp::unique_temp_path("codex-lock");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("owner");
    let lock = acquire_launch_lock(&path).unwrap();
    // A single child owns the inherited descriptor. Reaping a supervisor is
    // not a barrier for its descendants' descriptor teardown.
    let mut command = Command::new("/bin/sleep");
    command.arg("60").process_group(0);
    inherit_writer_lock(&mut command, &lock);
    let mut child = command.spawn().unwrap();
    drop(lock);
    let second = acquire_launch_lock(&path);
    let exclusive = matches!(second,Err(ref e) if e.kind()==io::ErrorKind::WouldBlock);
    drop(second);
    drop(child.stdin.take());
    stop(&mut child);
    let released = acquire_launch_lock(&path).is_ok();
    fs::remove_dir_all(dir).unwrap();
    assert!(
        exclusive,
        "a second runtime must not acquire the same launch while its writer survives"
    );
    assert!(
        released,
        "a stopped runtime must permit the validated cold resume"
    );
}

#[test]
fn codex_native_attached_profile_retains_the_configured_tui_without_starting_a_server() {
    let dir = crate::utils::test_temp::unique_temp_path("codex-profile");
    fs::create_dir_all(&dir).unwrap();
    let mut runtime = Runtime {
        config: Some(Config {
            env: HashMap::from([(
                "BUS_CALLBACK_DIR".into(),
                dir.to_string_lossy().into_owned(),
            )]),
            cwd: dir.clone(),
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
            env: HashMap::from([(
                "BUS_CALLBACK_DIR".into(),
                dir.to_string_lossy().into_owned(),
            )]),
            cwd: dir.clone(),
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
