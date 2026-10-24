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
