fn yolo_args(input: &[&str]) -> Vec<String> {
    input.iter().map(|value| (*value).into()).collect()
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
        let mut argv = yolo_args(&input);
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
        let mut normal = yolo_args(&input);
        launch::apply_run_permissions(ProviderKind::Codex, &mut normal);
        assert_eq!(normal, yolo_args(&input));
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
    let mut argv = yolo_args(&[
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

// Approval selectors after positionals, greedy
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
        let mut argv = yolo_args(&input);
        launch::apply_run_permissions(ProviderKind::Codex, &mut argv);
        if argv != yolo_args(&want) {
            failures.push(format!("{input:?} -> {argv:?}, want {want:?}"));
        }
    }
    std::env::remove_var("BUS_YOLO");
    if let Some(old) = old {
        std::env::set_var("BUS_YOLO", old);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
