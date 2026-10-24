//! Help must work before any session or transport setup.
use std::process::Command;

fn help(path: &[String], flag: &str) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_bus"))
        .arg("--dev")
        .args(path)
        .arg(flag)
        .env("BUS_DATA_DIR", "relative-help-must-not-resolve-a-session")
        .output()
        .unwrap();
    assert!(output.status.success(), "{path:?} {flag}: {:?}", output);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Usage:"), "{path:?}: {text}");
    assert!(text.contains("Example:"), "{path:?}: {text}");
    assert!(output.stderr.is_empty(), "{path:?}: {:?}", output.stderr);
    text
}

fn walk(path: &[String], count: &mut usize) {
    let text = help(path, "--help");
    help(path, "-h");
    *count += 1;
    let children: Vec<_> = text
        .lines()
        .skip_while(|line| *line != "Commands:")
        .skip(1)
        .take_while(|line| line.starts_with("  "))
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    for child in children {
        let mut path = path.to_vec();
        path.push(child.to_owned());
        walk(&path, count);
    }
}

#[test]
fn every_command_and_subcommand_help_exits_successfully_without_a_session() {
    let mut count = 0;
    walk(&[], &mut count);
    assert!(count >= 40, "walked only {count} commands");
}

#[test]
fn bare_groups_list_their_subcommands() {
    for group in ["agent", "room", "message", "request"] {
        let output = Command::new(env!("CARGO_BIN_EXE_bus"))
            .arg(group)
            .env("BUS_DATA_DIR", "relative-help-root")
            .output()
            .unwrap();
        assert!(output.status.success(), "{group}: {output:?}");
        assert!(String::from_utf8(output.stdout)
            .unwrap()
            .contains("Commands:"));
    }
}

#[test]
fn unexpected_argument_names_the_flag_and_points_to_help() {
    let root = std::env::temp_dir().join(format!("bus-help-error-{}", std::process::id()));
    let output = Command::new(env!("CARGO_BIN_EXE_bus"))
        .args(["agent", "add", "--bogus"])
        .env("BUS_DATA_DIR", &root)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let reply: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(reply["error"]["code"], "invalid_arguments");
    let message = reply["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("--bogus") && message.contains("--help"),
        "{message}"
    );
}
