use super::{args, parse_invocation, Action, ResumeTarget};

#[test]
fn yolo_is_not_inherited_by_a_later_public_invocation() {
    let _lock = crate::utils::config::test_config_env_lock().lock().unwrap();
    let old = std::env::var_os("BUS_YOLO");
    std::env::set_var("BUS_YOLO", "1");
    crate::cli::run(&args(&["--help"])).unwrap();
    let retained = std::env::var_os("BUS_YOLO");
    std::env::remove_var("BUS_YOLO");
    if let Some(old) = old {
        std::env::set_var("BUS_YOLO", old);
    }
    assert!(
        retained.is_none(),
        "a nested/later Bus must explicitly opt in"
    );
}

#[test]
fn yolo_startup_flag_accepts_fresh_and_resume_positions() {
    for input in [
        vec!["--yolo"],
        vec!["--dev", "--yolo"],
        vec!["--yolo", "resume", "--last"],
        vec!["resume", "--last", "--yolo"],
        vec!["--dev", "resume", "--yolo", "0123456789abcdef"],
    ] {
        assert!(parse_invocation(&args(&input)).is_ok(), "{input:?}");
    }
    assert_eq!(
        parse_invocation(&args(&["--yolo"])).unwrap().action,
        Action::Run
    );
    assert_eq!(
        parse_invocation(&args(&["resume", "--last", "--yolo"]))
            .unwrap()
            .action,
        Action::Resume(ResumeTarget::Last)
    );
}

#[test]
fn yolo_startup_flag_does_not_consume_agent_launch_arguments() {
    let invocation =
        parse_invocation(&args(&["--yolo", "agent", "add", "--args", "--yolo"])).unwrap();
    assert_eq!(
        invocation.action,
        Action::Control(args(&["agent", "add", "--args", "--yolo"]))
    );
}

// Codex 0.163.0 rejects `--dangerously-bypass-approvals-and-sandbox` (alias
// `--yolo`) together with `-a/--ask-for-approval`, for both `codex` and
// `codex resume`: "the argument ... cannot be used with ...", exit 2.
#[test]
fn yolo_codex_bypass_is_not_combined_with_an_explicit_approval_policy() {
    use crate::agents::providers::{launch, ProviderKind};
    let _lock = crate::utils::config::test_config_env_lock().lock().unwrap();
    let old = std::env::var_os("BUS_YOLO");
    std::env::set_var("BUS_YOLO", "1");
    let mut failures = Vec::new();
    for input in [
        vec!["-a", "on-request"],
        vec!["--ask-for-approval", "never"],
        vec!["--ask-for-approval=on-request"],
        vec!["-anever"],
        vec!["-a=on-request"],
        vec!["--approve-for-me"],
    ] {
        let mut argv = args(&input);
        launch::apply_run_permissions(ProviderKind::Codex, &mut argv);
        let bypass = argv
            .iter()
            .any(|a| a == "--yolo" || a == "--dangerously-bypass-approvals-and-sandbox");
        let approval = argv.iter().any(|a| {
            a == "-a" || a == "--ask-for-approval" || a.starts_with("--ask-for-approval=")
        });
        if !bypass
            || approval
            || argv
                .iter()
                .any(|a| a == "--approve-for-me" || a.starts_with("-a"))
        {
            failures.push(format!("{input:?} -> {argv:?}"));
        }
        std::env::remove_var("BUS_YOLO");
        let mut normal = args(&input);
        launch::apply_run_permissions(ProviderKind::Codex, &mut normal);
        assert_eq!(normal, args(&input));
        std::env::set_var("BUS_YOLO", "1");
    }
    std::env::remove_var("BUS_YOLO");
    if let Some(old) = old {
        std::env::set_var("BUS_YOLO", old);
    }
    assert!(
        failures.is_empty(),
        "Codex refuses to start with both flags:\n{}",
        failures.join("\n")
    );
}

#[test]
fn yolo_codex_preserves_compatible_options_and_literal_approval_values() {
    use crate::agents::providers::{launch, ProviderKind};
    let _lock = crate::utils::config::test_config_env_lock().lock().unwrap();
    let old = std::env::var_os("BUS_YOLO");
    std::env::set_var("BUS_YOLO", "1");
    let mut argv = args(&[
        "--future-option",
        "--config",
        "--ask-for-approval",
        "--sandbox",
        "workspace-write",
        "--yolo",
        "--",
        "--approve-for-me",
    ]);
    let original = argv.clone();
    launch::apply_run_permissions(ProviderKind::Codex, &mut argv);
    std::env::remove_var("BUS_YOLO");
    if let Some(old) = old {
        std::env::set_var("BUS_YOLO", old);
    }
    assert_eq!(argv, original);
}

#[test]
fn yolo_startup_flag_is_recorded_in_every_accepted_position() {
    for input in [
        vec!["--yolo"],
        vec!["--dev", "--yolo"],
        vec!["--yolo", "resume", "--last"],
        vec!["resume", "--last", "--yolo"],
        vec!["--dev", "resume", "--yolo", "0123456789abcdef"],
    ] {
        assert!(parse_invocation(&args(&input)).unwrap().yolo, "{input:?}");
    }
    for input in [
        vec![],
        vec!["resume", "--last"],
        vec!["agent", "add", "--args", "--yolo"],
    ] {
        assert!(!parse_invocation(&args(&input)).unwrap().yolo, "{input:?}");
    }
}

// Review probe (claude-yolo R2): approval selectors after positionals, greedy
// image values and unmodeled options are still removed from yolo argv only.
#[test]
fn yolo_codex_removes_approval_after_subcommand_images_and_unknown_options() {
    use crate::agents::providers::{launch, ProviderKind};
    let _lock = crate::utils::config::test_config_env_lock().lock().unwrap();
    let old = std::env::var_os("BUS_YOLO");
    std::env::set_var("BUS_YOLO", "1");
    let cases = [
        (
            vec![
                "resume",
                "019a0000-0000-7000-8000-000000000000",
                "-a",
                "never",
            ],
            vec!["--yolo", "resume", "019a0000-0000-7000-8000-000000000000"],
        ),
        (
            vec![
                "-i",
                "a.png",
                "b.png",
                "--approve-for-me",
                "-s",
                "read-only",
            ],
            vec!["--yolo", "-i", "a.png", "b.png", "-s", "read-only"],
        ),
        (
            vec!["--future-flag", "--ask-for-approval", "never", "-m", "-a"],
            vec!["--yolo", "--future-flag", "-m", "-a"],
        ),
        (
            vec!["-a", "never", "--", "-a", "never"],
            vec!["--yolo", "--", "-a", "never"],
        ),
    ];
    let mut failures = Vec::new();
    for (input, want) in cases {
        let mut argv = args(&input);
        launch::apply_run_permissions(ProviderKind::Codex, &mut argv);
        if argv != args(&want) {
            failures.push(format!("{input:?} -> {argv:?}, want {want:?}"));
        }
    }
    std::env::remove_var("BUS_YOLO");
    if let Some(old) = old {
        std::env::set_var("BUS_YOLO", old);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
