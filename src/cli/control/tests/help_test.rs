use super::parse;

fn walk(command: &clap::Command, path: &[String]) {
    for flag in ["--help", "-h"] {
        let mut args = path.to_vec();
        args.push(flag.into());
        let error = parse::cli().try_get_matches_from(&args).unwrap_err();
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
}

#[test]
fn unexpected_argument_names_the_argument_and_help() {
    let args = ["agent", "add", "--bogus"].map(String::from);
    let error = parse::parse(&args, "help-test").unwrap_err();
    assert!(error.contains("--bogus"), "{error}");
    assert!(error.contains("--help"), "{error}");
}

#[test]
fn command_help_matches_process_flag_positions() {
    let args = ["agent", "add", "--help"].map(String::from);
    let text = super::help_for(super::command_tree(false), &args).unwrap();
    assert!(
        !text.contains("--dev") && !text.contains("--yolo"),
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
