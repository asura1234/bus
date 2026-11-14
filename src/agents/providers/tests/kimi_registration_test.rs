use super::kimi::wire::committed_final;
use super::{launch, parse, spool::Parsed, ProviderKind};

#[test]
fn kimi_hooks_bind_session_and_started_turn_without_treating_stop_as_final() {
    let session = serde_json::json!({"hook_event_name":"SessionStart","session_id":"session_abc","source":"startup"});
    assert_eq!(
        parse(ProviderKind::Kimi, &session).unwrap(),
        Parsed::Session {
            session: "session_abc".into(),
            source: Some("startup".into())
        }
    );
    let start = serde_json::json!({"hook_event_name":"TurnStarted","session_id":"session_abc","turn_id":7,"origin_kind":"user","prompt":"Bus task"});
    assert_eq!(
        parse(ProviderKind::Kimi, &start).unwrap(),
        Parsed::Started {
            session: "session_abc".into(),
            turn: "7".into(),
            prompt: "Bus task".into()
        }
    );
    let stop = serde_json::json!({"hook_event_name":"Stop","session_id":"session_abc","stop_hook_active":false});
    assert_eq!(
        parse(ProviderKind::Kimi, &stop).unwrap(),
        Parsed::ReconcileSession {
            session: "session_abc".into()
        }
    );
}

#[test]
fn kimi_optional_session_does_not_swallow_a_following_flag() {
    let keep_flag = vec![
        "--session".into(),
        "-y".into(),
        "--model".into(),
        "synthetic".into(),
    ];
    assert_eq!(
        super::kimi::launch::resumable_options(&keep_flag),
        vec!["-y", "--model", "synthetic"]
    );
    let keep_id = vec![
        "--session".into(),
        "sid".into(),
        "--model".into(),
        "synthetic".into(),
    ];
    assert_eq!(
        super::kimi::launch::resumable_options(&keep_id),
        vec!["--model", "synthetic"]
    );
    assert_eq!(
        super::kimi::launch::resumable_options(&["-S".into(), "-y".into()]),
        vec!["-y"]
    );
    assert_eq!(
        super::kimi::launch::resumable_options(&[
            "-S".into(),
            "sid".into(),
            "--model".into(),
            "synthetic".into()
        ]),
        vec!["--model", "synthetic"]
    );
}

#[test]
fn kimi_resume_options_retain_yolo_but_drop_new_session_file() {
    let options = vec![
        "--yolo".into(),
        "--agent-file".into(),
        "/private/agent.md".into(),
    ];
    assert_eq!(
        launch::resumable_options(ProviderKind::Kimi, &options),
        vec!["--yolo"]
    );
    assert_eq!(
        super::kimi::launch::resume_args("session_abc"),
        vec!["--session", "session_abc"]
    );
}

#[test]
fn completed_main_turn_publishes_only_its_final_text() {
    let wire = concat!(
        "{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":7,\"promptId\":\"prompt-7\",\"input\":[{\"type\":\"text\",\"text\":\"Bus task\"}]}\n",
        "{\"type\":\"agent.message.appended\",\"message\":{\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"think\",\"think\":\"private\"},{\"type\":\"text\",\"text\":\"Final answer\"}],\"toolCalls\":[]},\"meta\":{\"source\":\"llm\"}}}\n",
        "{\"type\":\"agent.turn.ended\",\"turnId\":7,\"outcome\":\"done\"}\n",
        "{\"type\":\"turn.ended\",\"agentId\":\"main\",\"turnId\":7,\"reason\":\"completed\"}\n",
        "{\"type\":\"prompt.completed\",\"agentId\":\"main\",\"promptId\":\"prompt-7\",\"reason\":\"completed\"}\n",
    );
    assert_eq!(
        committed_final(wire, 7, "Bus task").as_deref(),
        Some("Final answer")
    );
    assert_eq!(committed_final(wire, 7, "Other task"), None);
}

#[test]
fn stop_or_partial_wire_never_claims_a_reply() {
    let partial = concat!(
        "{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":3,\"promptId\":\"prompt-3\",\"input\":[{\"type\":\"text\",\"text\":\"Task\"}]}\n",
        "{\"type\":\"agent.message.appended\",\"message\":{\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Premature\"}],\"toolCalls\":[]},\"meta\":{\"source\":\"llm\"}}}\n",
        "{\"type\":\"agent.turn.ended\",\"turnId\":3,\"outcome\":\"done\"}\n",
    );
    assert_eq!(committed_final(partial, 3, "Task"), None);
    assert_eq!(
        committed_final(&(partial.to_owned() + "{\"type\":\"turn.end"), 3, "Task"),
        None
    );
}

#[test]
fn interrupted_turn_never_claims_visible_assistant_text() {
    let wire = concat!(
        "{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":4,\"promptId\":\"prompt-4\",\"input\":[{\"type\":\"text\",\"text\":\"Task\"}]}\n",
        "{\"type\":\"agent.message.appended\",\"message\":{\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Interrupted answer\"}],\"toolCalls\":[]},\"meta\":{\"source\":\"llm\"}}}\n",
        "{\"type\":\"agent.turn.ended\",\"turnId\":4,\"outcome\":\"interrupted\"}\n",
        "{\"type\":\"turn.ended\",\"agentId\":\"main\",\"turnId\":4,\"reason\":\"interrupted\"}\n",
        "{\"type\":\"prompt.completed\",\"agentId\":\"main\",\"promptId\":\"prompt-4\",\"reason\":\"interrupted\"}\n",
    );
    assert_eq!(committed_final(wire, 4, "Task"), None);
}

#[test]
fn assistant_text_after_turn_end_cannot_replace_the_final() {
    let wire = concat!(
        "{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":9,\"promptId\":\"prompt-9\",\"input\":[{\"type\":\"text\",\"text\":\"Task\"}]}\n",
        "{\"type\":\"agent.message.appended\",\"message\":{\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Correct\"}],\"toolCalls\":[]},\"meta\":{\"source\":\"llm\"}}}\n",
        "{\"type\":\"agent.turn.ended\",\"turnId\":9,\"outcome\":\"done\"}\n",
        "{\"type\":\"turn.ended\",\"agentId\":\"main\",\"turnId\":9,\"reason\":\"completed\"}\n",
        "{\"type\":\"agent.message.appended\",\"message\":{\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Injected\"}],\"toolCalls\":[]},\"meta\":{\"source\":\"llm\"}}}\n",
        "{\"type\":\"prompt.completed\",\"agentId\":\"main\",\"promptId\":\"prompt-9\",\"reason\":\"completed\"}\n",
    );
    assert_eq!(committed_final(wire, 9, "Task").as_deref(), Some("Correct"));
}

#[test]
fn kimi_yolo_normalizes_resume_argv_and_replays_saved_options_normally() {
    struct Restore(Option<std::ffi::OsString>);
    impl Drop for Restore {
        fn drop(&mut self) {
            match &self.0 {
                Some(value) => std::env::set_var("BUS_YOLO", value),
                None => std::env::remove_var("BUS_YOLO"),
            }
        }
    }
    let _guard = crate::utils::config::test_config_env_lock().lock().unwrap();
    let restore = Restore(std::env::var_os("BUS_YOLO"));
    for (saved, yolo, normal) in [
        (vec![], vec!["--auto"], vec![]),
        (vec!["--auto"], vec!["--auto"], vec!["--auto"]),
        (vec!["--yolo"], vec!["--auto"], vec!["--yolo"]),
        (vec!["-y"], vec!["--auto"], vec!["-y"]),
        (vec!["-yy"], vec!["--auto"], vec!["-yy"]),
        (
            vec!["-ymVALUE"],
            vec!["--auto", "-mVALUE"],
            vec!["-ymVALUE"],
        ),
        (
            vec!["-ymcustom", "--yolo"],
            vec!["--auto", "-mcustom"],
            vec!["-ymcustom", "--yolo"],
        ),
        (
            vec!["-ym", "VALUE"],
            vec!["--auto", "-m", "VALUE"],
            vec!["-ym", "VALUE"],
        ),
        (
            vec!["--model", "--yolo", "--output-format", "-y"],
            vec!["--auto", "--model", "--yolo", "--output-format", "-y"],
            vec!["--model", "--yolo", "--output-format", "-y"],
        ),
        (
            vec!["--auto", "--yolo"],
            vec!["--auto"],
            vec!["--auto", "--yolo"],
        ),
        (vec!["-yS", "-y"], vec!["--auto", "-S"], vec!["-yS", "-y"]),
    ] {
        for (enabled, expected) in [(true, yolo), (false, normal)] {
            if enabled {
                std::env::set_var("BUS_YOLO", "1");
            } else {
                std::env::remove_var("BUS_YOLO");
            }
            let saved: Vec<String> = saved.iter().map(|arg| (*arg).to_owned()).collect();
            let mut args = super::kimi::launch::resumable_options(&saved);
            launch::apply_run_permissions(ProviderKind::Kimi, &mut args);
            let validated = super::kimi::launch::validate_run_permissions(&args);
            if !enabled && saved == ["--auto", "--yolo"] {
                assert!(
                    validated.is_err(),
                    "native mixed modes reached Kimi: {args:?}"
                );
            } else {
                validated.unwrap();
            }
            assert_eq!(args, expected, "saved={saved:?}, yolo={enabled}");
        }
    }
    drop(restore);
}

#[test]
fn kimi_mixed_mode_guard_checks_only_option_positions() {
    let values = vec![
        "--model".into(),
        "--auto".into(),
        "--output-format".into(),
        "--yolo".into(),
        "--".into(),
        "-y".into(),
    ];
    super::kimi::launch::validate_run_permissions(&values).unwrap();
    for args in [
        vec!["--auto", "--yolo"],
        vec!["-y", "--auto"],
        vec!["--auto", "-yy"],
        vec!["-yy", "--auto"],
        vec!["--auto", "-ymVALUE"],
        vec!["-ymVALUE", "--auto"],
        vec!["--auto", "-ym", "VALUE"],
    ] {
        let args: Vec<String> = args.into_iter().map(str::to_owned).collect();
        assert!(super::kimi::launch::validate_run_permissions(&args).is_err());
    }
}

#[test]
fn launch_rejects_positional_input_after_the_option_terminator() {
    let args = vec!["--".to_owned(), "unowned task".to_owned()];
    assert!(
        super::kimi::launch::validate_args(&args).is_err(),
        "a positional provider prompt must not bypass Bus ownership: {args:?}"
    );
}

#[test]
fn launch_rejects_attached_session_and_prompt_short_options() {
    for arg in ["-Sexternal-session", "-punowned task"] {
        let args = vec![arg.to_owned()];
        assert!(
            super::kimi::launch::validate_args(&args).is_err(),
            "Kimi accepts this attached short option; Bus must reject it: {arg}"
        );
        if arg.starts_with("-S") {
            assert!(
                super::kimi::launch::take_adopted_session(&args).is_err(),
                "an attached session selector must not bypass adoption refusal: {arg}"
            );
        }
    }
}

#[test]
fn native_kimi_accepts_normalized_short_option_combinations() {
    let binary = std::env::var("BUS_KIMI_NATIVE_BINARY").ok();
    struct Restore(Option<std::ffi::OsString>);
    impl Drop for Restore {
        fn drop(&mut self) {
            match &self.0 {
                Some(value) => std::env::set_var("BUS_YOLO", value),
                None => std::env::remove_var("BUS_YOLO"),
            }
        }
    }
    let _guard = crate::utils::config::test_config_env_lock().lock().unwrap();
    let _restore = Restore(std::env::var_os("BUS_YOLO"));
    let scratch = binary.as_ref().map(|_| {
        let root = crate::utils::test_temp::unique_temp_path("kimi-native-yolo-options");
        let home = root.join("home");
        let kimi_home = root.join("kimi-home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&kimi_home).unwrap();
        (root, home, kimi_home)
    });
    fn random(seed: &mut u64) -> usize {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (*seed >> 32) as usize
    }
    let mut seed = 0x4b494d495f594f4c;
    for case in 0..48 {
        let model = ["alpha", "custom", "beta", "gamma"][random(&mut seed) % 4];
        let y = "y".repeat(1 + random(&mut seed) % 3);
        let mut units = Vec::new();
        if random(&mut seed).is_multiple_of(3) {
            if random(&mut seed).is_multiple_of(2) {
                units.push(vec![format!("-{y}m{model}")]);
            } else {
                units.push(vec![format!("-{y}m"), model.to_owned()]);
            }
        } else {
            units.push(vec![if random(&mut seed).is_multiple_of(2) {
                "--yolo".to_owned()
            } else {
                format!("-{y}")
            }]);
            units.push(match random(&mut seed) % 4 {
                0 => vec!["--model".into(), model.into()],
                1 => vec![format!("--model={model}")],
                2 => vec!["-m".into(), model.into()],
                _ => vec![format!("-m{model}")],
            });
        }
        if random(&mut seed).is_multiple_of(2) {
            units.push(vec![
                "--output-format".into(),
                if random(&mut seed).is_multiple_of(2) {
                    "text"
                } else {
                    "stream-json"
                }
                .into(),
            ]);
        }
        if random(&mut seed).is_multiple_of(2) {
            units.push(vec!["--add-dir".into(), "/tmp".into()]);
        }
        if random(&mut seed).is_multiple_of(2) {
            units.push(vec!["--skills-dir".into(), "/tmp".into()]);
        }
        if random(&mut seed).is_multiple_of(2) {
            units.push(vec!["--auto".into()]);
        }
        for index in (1..units.len()).rev() {
            units.swap(index, random(&mut seed) % (index + 1));
        }
        let saved: Vec<String> = units.into_iter().flatten().collect();
        super::kimi::launch::validate_owned_launch(&saved).unwrap();
        std::env::remove_var("BUS_YOLO");
        let mut normal = saved.clone();
        launch::apply_run_permissions(ProviderKind::Kimi, &mut normal);
        assert_eq!(normal, saved, "normal replay changed case {case}");
        assert_eq!(super::kimi::launch::resumable_options(&saved), saved);

        std::env::set_var("BUS_YOLO", "1");
        let mut fresh = saved.clone();
        launch::apply_run_permissions(ProviderKind::Kimi, &mut fresh);
        let mut resumed = super::kimi::launch::resumable_options(&saved);
        launch::apply_run_permissions(ProviderKind::Kimi, &mut resumed);
        assert_eq!(fresh, resumed, "fresh/resume mismatch in case {case}");
        assert_eq!(fresh.iter().filter(|arg| *arg == "--auto").count(), 1);
        assert!(!fresh
            .iter()
            .any(|arg| super::kimi::launch::is_yolo_option(arg)));
        super::kimi::launch::validate_run_permissions(&fresh).unwrap();
        super::kimi::launch::validate_run_permissions(&resumed).unwrap();
        // Kimi 2.4.1 accepts `--output-format` only with `--prompt`, which Bus
        // rejects. Keep the Bus asserts above and start the live process only
        // for argv an interactive Kimi can accept.
        let prompt_only_output = fresh
            .iter()
            .any(|arg| arg == "--output-format" || arg.starts_with("--output-format="));
        if !prompt_only_output {
            if let Some((binary, (_, home, kimi_home))) = binary.as_deref().zip(scratch.as_ref()) {
                // `--help` is printed before option checks, so it accepts illegal argv.
                // A real startup that exits is the parse or permission failure.
                assert_native_kimi_stays_alive(binary, &fresh, home, kimi_home, case, &saved);
            }
        }
    }
    if let Some((root, _, _)) = scratch {
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn kimi_yolo_normalization_replaces_hidden_yolo_aliases() {
    let _guard = crate::utils::config::test_config_env_lock().lock().unwrap();
    struct Restore(Option<std::ffi::OsString>);
    impl Drop for Restore {
        fn drop(&mut self) {
            match &self.0 {
                Some(value) => std::env::set_var("BUS_YOLO", value),
                None => std::env::remove_var("BUS_YOLO"),
            }
        }
    }
    let _restore = Restore(std::env::var_os("BUS_YOLO"));
    std::env::set_var("BUS_YOLO", "1");
    let mut failures = Vec::new();
    for alias in ["--yes", "--auto-approve"] {
        let mut effective = vec![alias.to_string()];
        launch::apply_run_permissions(ProviderKind::Kimi, &mut effective);
        if effective != ["--auto".to_string()] {
            failures.push(format!(
                "Bus yolo left Kimi yolo alias {alias} beside --auto: {effective:?}"
            ));
        }
        for mixed in [
            vec!["--auto".to_string(), alias.to_string()],
            vec![alias.to_string(), "--auto".to_string()],
        ] {
            if super::kimi::launch::validate_run_permissions(&mixed).is_ok() {
                failures.push(format!("guard saved a normal-resume rejection: {mixed:?}"));
            }
        }
    }
    let mut clustered = vec!["-yr".to_string()];
    launch::apply_run_permissions(ProviderKind::Kimi, &mut clustered);
    if clustered != ["--auto".to_string(), "-r".to_string()] {
        failures.push(format!(
            "Bus yolo left a yolo bit inside an unrecognized short cluster: {clustered:?}"
        ));
    }
    for mixed in [
        vec!["--auto".to_string(), "-yr".to_string()],
        vec!["-yr".to_string(), "--auto".to_string()],
    ] {
        if super::kimi::launch::validate_run_permissions(&mixed).is_ok() {
            failures.push(format!("guard saved a normal-resume rejection: {mixed:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("; "));
}

fn assert_native_kimi_stays_alive(
    binary: &str,
    args: &[String],
    home: &std::path::Path,
    kimi_home: &std::path::Path,
    case: usize,
    saved: &[String],
) {
    let mut child = std::process::Command::new(binary)
        .args(args)
        .env_clear()
        .envs([
            ("HOME", home.as_os_str()),
            ("KIMI_CODE_HOME", kimi_home.as_os_str()),
            ("PATH", std::ffi::OsStr::new("/usr/bin:/bin")),
            ("KIMI_CODE_NO_AUTO_UPDATE", std::ffi::OsStr::new("1")),
            ("KIMI_DISABLE_OAUTH_LOCK", std::ffi::OsStr::new("0")),
            ("TERM", std::ffi::OsStr::new("dumb")),
            ("NO_COLOR", std::ffi::OsStr::new("1")),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("native Kimi failed to start: {error}"));
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut byte = [0u8; 1];
        let read = std::io::Read::read(&mut stdout, &mut byte).unwrap_or(0);
        let _ = started_tx.send(read);
    });
    let stderr_thread = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = std::io::Read::read_to_end(&mut stderr, &mut bytes);
        bytes
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if let Ok(read) = started_rx.try_recv() {
            if let Some(status) = child.try_wait().unwrap() {
                break Some(status);
            }
            if read > 0 {
                break None;
            }
        }
        if std::time::Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let _ = child.kill();
    let _ = child.wait();
    let stderr_bytes = stderr_thread.join().unwrap_or_default();
    if let Some(status) = status {
        panic!(
            "native Kimi rejected case {case}: status={status}, saved={saved:?}, effective={args:?}, stderr={}",
            String::from_utf8_lossy(&stderr_bytes)
        );
    }
}
