use super::*;

#[test]
fn terminal_driver_commands_are_available_without_dev_and_preserve_typed_input() {
    let observe = command(&["agent", "observe", "w1:p2", "--pane"]).unwrap();
    assert_eq!(observe.method, "agent.observe");
    assert_eq!(observe.params, json!({"agent":"w1:p2", "pane":true}));
    let press = command(&[
        "agent",
        "press",
        "worker",
        "--text",
        "  A typed answer  ",
        "--key",
        "ctrl+l",
        "--key",
        "2",
        "--submit",
        "--terminal-id",
        "term_2",
    ])
    .unwrap();
    assert_eq!(press.method, "agent.press");
    assert_eq!(
        press.params,
        json!({"agent":"worker", "text":"  A typed answer  ", "keys":["ctrl+l", "2"], "submit":true, "terminal_id":"term_2"})
    );
    assert_eq!(
        command(&["agent", "press", "worker", "--text", ""])
            .unwrap()
            .params["text"],
        ""
    );
    for key in ["y", "n", "q", "ctrl+c", "ctrl+d", "ctrl+z"] {
        assert!(command(&["agent", "press", "w1:p2", "--pane", "--key", key]).is_ok());
    }
}

#[test]
fn terminal_driver_selection_is_explicit_and_does_not_mix_with_other_input() {
    let select = command(&[
        "agent",
        "press",
        "worker",
        "--select",
        "Continue with current version",
    ])
    .unwrap();
    assert_eq!(
        select.params,
        json!({"agent":"worker", "select":"Continue with current version"})
    );
    assert!(command(&["agent", "press", "worker"]).is_err());
    for extra in [
        vec!["--key", "enter"],
        vec!["--text", "answer"],
        vec!["--submit"],
    ] {
        let mut args = vec!["agent", "press", "worker", "--select", "Continue"];
        args.extend(extra);
        assert!(command(&args).is_err());
    }
    let help = command_help(&["agent", "press"]);
    assert!(help.contains("oh-my-zsh"));
    assert!(help.contains("--text"));
    assert!(help.contains("--pane"));
}
