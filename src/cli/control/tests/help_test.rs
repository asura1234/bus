use super::parse;

fn walk(command: &clap::Command, path: &[String]) {
    let long = command
        .get_long_about()
        .expect("every command needs long help");
    assert!(!long.to_string().trim().is_empty(), "{path:?}");
    assert!(
        !long.to_string().contains("Missing command documentation"),
        "{path:?}"
    );
    for flag in ["--help", "-h"] {
        let mut args = path.to_vec();
        args.push(flag.into());
        let error = super::command_tree(true)
            .try_get_matches_from(&args)
            .unwrap_err();
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::DisplayHelp,
            "{args:?}: {error}"
        );
        assert!(error.to_string().contains("Usage:"), "{args:?}: {error}");
    }
    for child in command.get_subcommands() {
        let mut child_path = path.to_vec();
        child_path.push(child.get_name().into());
        walk(child, &child_path);
    }
}

#[test]
fn every_control_command_accepts_help() {
    walk(&parse::cli(), &[]);
    walk(&super::command_tree(true), &[]);
}

#[test]
fn control_long_help_documents_results_errors_examples_and_defaults() {
    fn check(command: &clap::Command, path: &str) {
        let long = command.get_long_about().unwrap().to_string();
        assert!(long.contains("JSON and common errors:"), "{path}");
        if command.get_subcommands().next().is_none() {
            assert!(long.contains("Result:"), "{path}: missing result schema");
            assert!(long.contains("Errors:"), "{path}: missing error recovery");
        }
        assert!(
            command
                .get_after_help()
                .unwrap()
                .to_string()
                .contains("Example:"),
            "{path}"
        );
        for child in command.get_subcommands() {
            check(child, &format!("{path} {}", child.get_name()));
        }
    }
    check(&parse::cli(), "bus");
}

#[test]
fn unexpected_argument_names_the_argument_and_help() {
    let args = ["agent", "add", "--bogus"].map(String::from);
    let error = parse::parse(&args, "help-test").unwrap_err();
    assert!(error.contains("--bogus"), "{error}");
    assert!(error.contains("--help"), "{error}");
}

#[test]
fn interrupt_help_uses_ctrl_c_for_grok_and_esc_for_kimi() {
    let args = ["agent", "interrupt", "--help"].map(String::from);
    let text = super::help_for(super::command_tree(false), &args).unwrap();
    assert!(
        text.contains("Esc for Claude/Codex/Kimi") && text.contains("Ctrl+C for Cursor and Grok"),
        "{text}"
    );
}

#[test]
fn command_help_matches_process_flag_positions() {
    let args = ["agent", "add", "--help"].map(String::from);
    let text = super::help_for(super::command_tree(false), &args).unwrap();
    assert!(
        !text.contains("      --dev") && !text.contains("      --yolo"),
        "{text}"
    );
    let misplaced = ["agent", "add", "--dev", "--help"].map(String::from);
    assert!(super::help_for(super::command_tree(false), &misplaced).is_none());
    assert!(parse::parse(&misplaced, "id")
        .unwrap_err()
        .contains("--dev"));
    for args in [
        vec!["--dev", "agent", "--help"],
        vec!["--yolo", "--dev", "agent", "--help"],
        vec!["resume", "--dev", "--help"],
    ] {
        let args: Vec<_> = args.into_iter().map(String::from).collect();
        assert!(
            crate::cli::help::subcommand_help(&args).is_some(),
            "{args:?}"
        );
    }
}

#[test]
fn local_help_includes_accepted_trailing_run_flags() {
    use crate::cli::session_pick::{parse_invocation, Action};
    for (command, action) in [("sessions", Action::Sessions), ("stop", Action::Stop)] {
        let args = [command, "--help"].map(String::from);
        let help = super::help_for(super::command_tree(false), &args).unwrap();
        for flag in ["--dev", "--yolo"] {
            let invocation = parse_invocation(&[command, flag].map(String::from)).unwrap();
            assert_eq!(invocation.action, action);
            assert!(if flag == "--dev" {
                invocation.dev
            } else {
                invocation.yolo
            });
            assert!(
                help.contains(flag),
                "{command} accepts {flag} but help omits it: {help}"
            );
        }
    }
}

fn collect_fallbacks(command: &clap::Command, path: &str, found: &mut Vec<String>) {
    if command
        .get_about()
        .is_some_and(|about| about.to_string() == "Bus command")
    {
        found.push(format!("{path}: about"));
    }
    for arg in command.get_arguments() {
        if arg
            .get_help()
            .is_some_and(|help| help.to_string() == "Command argument")
        {
            found.push(format!("{path}: --{}", arg.get_id()));
        }
    }
    for child in command.get_subcommands() {
        collect_fallbacks(child, &format!("{path} {}", child.get_name()), found);
    }
}

#[test]
fn every_command_and_flag_has_its_own_help_text() {
    for dev in [false, true] {
        let mut found = Vec::new();
        collect_fallbacks(&super::command_tree(dev), "bus", &mut found);
        assert!(found.is_empty(), "generic help placeholders: {found:?}");
    }
}

#[test]
fn bare_and_help_group_listings_hide_dev_subcommands() {
    for args in [
        &["room"][..],
        &["room", "--help"],
        &["agent"],
        &["agent", "--help"],
    ] {
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        let text = super::help_for(super::command_tree(false), &args).expect("group help");
        for dev in ["  focus", "  seen", "  sound", "  details"] {
            assert!(
                !text.contains(dev),
                "{args:?} advertises dev tool {dev:?}:\n{text}"
            );
        }
    }
}

#[test]
fn required_flags_and_optional_context_have_truthful_defaults() {
    fn check(command: &clap::Command, path: &str) {
        for arg in command.get_arguments().filter(|arg| arg.is_required_set()) {
            let help = arg.get_help().unwrap().to_string();
            assert!(
                !help.contains("[default:"),
                "{path} {}: {help}",
                arg.get_id()
            );
        }
        for child in command.get_subcommands() {
            check(child, &format!("{path} {}", child.get_name()));
        }
    }
    check(&super::command_tree(true), "bus");
    let args = ["agent", "read", "--help"].map(String::from);
    let help = super::help_for(super::command_tree(false), &args).unwrap();
    assert!(
        help.contains("default: none; optional conversation context"),
        "{help}"
    );
    assert!(help.contains("default: recent; requires --lines"), "{help}");
    let missing_author = ["message", "seen", "456"].map(String::from);
    assert_eq!(
        parse::cli()
            .try_get_matches_from(missing_author)
            .unwrap_err()
            .kind(),
        clap::error::ErrorKind::MissingRequiredArgument
    );
}

#[test]
fn declared_defaults_render_once_and_local_defaults_remain_documented() {
    let args = ["wait", "--help"].map(String::from);
    let help = super::help_for(super::command_tree(false), &args).unwrap();
    assert_eq!(help.matches("[default: 60]").count(), 1, "{help}");
    for (path, expected) in [
        (vec!["perf"], "default 30m"),
        (vec!["resume"], "default: false"),
        (vec!["sessions"], "default: false"),
        (vec!["stop"], "default: false"),
    ] {
        let args: Vec<String> = path
            .into_iter()
            .chain(["--help"])
            .map(String::from)
            .collect();
        let help = super::help_for(super::command_tree(false), &args).unwrap();
        assert!(help.contains(expected), "{args:?}: {help}");
    }
}

#[test]
fn agent_add_help_keeps_prompt_template_tokens_literal() {
    let args = ["agent", "add", "--help"].map(String::from);
    let help = super::help_for(super::command_tree(false), &args).unwrap();
    for token in [
        "{{ROOM_NAME}}",
        "{{ROOM_ID}}",
        "{{AGENT_NAME}}",
        "{{DOCS}}",
        "{{WORKFLOWS_DIR}}",
    ] {
        assert!(help.contains(token), "missing literal prompt token {token}");
    }
}
