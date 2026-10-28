use super::*;

#[test]
fn reply_diagnostics_hybrid_flags_never_send_and_show_the_reply_form() {
    for extra in [
        vec!["--room", "1"],
        vec!["--to", "orchestrator"],
        vec!["--room", "1", "--to", "orchestrator"],
        vec!["--queue"],
        vec!["--async"],
        vec!["--file", "/tmp/verdict.txt"],
    ] {
        let mut args = vec![
            "send",
            "--reply-to",
            "28089",
            "--as",
            "26457",
            "--text",
            "VERIFIED",
        ];
        args.extend(extra);
        let mut output = Vec::new();
        let result = run_with(
            &args.into_iter().map(String::from).collect::<Vec<_>>(),
            &mut output,
            |_, _| panic!("an invalid hybrid reply must never reach the server"),
        );
        assert!(result.is_err());
        let response: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(response["error"]["code"], "invalid_arguments");
        let error = response["error"]["message"].as_str().unwrap();
        assert!(error.contains("Nothing was sent"), "{error}");
        assert!(
            error.contains("bus send --reply-to 28089 --as=26457 --text '...'"),
            "{error}"
        );
        for flag in ["--room", "--to", "--queue", "--async", "--file"] {
            assert!(error.contains(flag), "{error}");
        }
    }
}

#[test]
fn reply_diagnostics_help_explains_when_a_reply_has_been_delivered() {
    let mut output = Vec::new();
    run_with(
        &["send", "--help"].map(String::from),
        &mut output,
        |_, _| panic!("help must not contact the server"),
    )
    .unwrap();
    let help = String::from_utf8(output).unwrap();
    assert!(help.contains("foreground"), "{help}");
    assert!(help.contains("stage:\"replied\""), "{help}");
    assert!(help.contains("before resending"), "{help}");
    assert!(
        help.contains("Excludes --room, --to, --queue, --async and --file"),
        "{help}"
    );
}

#[test]
fn reply_diagnostics_hybrid_without_text_still_shows_the_reply_form() {
    for extra in [
        vec!["--file", "/tmp/verdict.txt"],
        vec!["--room", "1", "--to", "orchestrator"],
    ] {
        let mut args = vec!["send", "--reply-to", "28089", "--as", "26457"];
        args.extend(extra);
        let mut output = Vec::new();
        let result = run_with(
            &args.into_iter().map(String::from).collect::<Vec<_>>(),
            &mut output,
            |_, _| panic!("an invalid hybrid reply must never reach the server"),
        );
        assert!(result.is_err());
        let response: Value = serde_json::from_slice(&output).unwrap();
        let error = response["error"]["message"].as_str().unwrap();
        assert!(error.contains("Nothing was sent"), "{error}");
        assert!(
            error.contains("bus send --reply-to 28089 --as=26457 --text '...'"),
            "{error}"
        );
    }
}

#[test]
fn reply_diagnostics_hybrid_quotes_an_agent_name_that_needs_shell_quoting() {
    let mut output = Vec::new();
    let _ = run_with(
        &[
            "send",
            "--reply-to",
            "28089",
            "--as",
            "it's me",
            "--text",
            "x",
            "--room",
            "1",
        ]
        .map(String::from),
        &mut output,
        |_, _| panic!("an invalid hybrid reply must never reach the server"),
    );
    let response: Value = serde_json::from_slice(&output).unwrap();
    let error = response["error"]["message"].as_str().unwrap();
    assert!(error.contains(r#"--as='it'\''s me' --text"#), "{error}");
}

#[test]
fn reply_diagnostics_suggested_command_preserves_leading_hyphen_author() {
    let valid = [
        "send",
        "--reply-to",
        "123",
        "--as=-worker",
        "--text",
        "done",
    ]
    .map(String::from);
    assert_eq!(
        parse(&valid, "valid-selector").unwrap().params["as"],
        "-worker"
    );
    let hybrid = [
        "send",
        "--reply-to",
        "123",
        "--as=-worker",
        "--room",
        "1",
        "--text",
        "done",
    ]
    .map(String::from);
    let mut output = Vec::new();
    assert!(run_with(&hybrid, &mut output, |_, _| {
        panic!("hybrid must not reach the coordinator")
    })
    .is_err());
    let response: Value = serde_json::from_slice(&output).unwrap();
    let message = response["error"]["message"].as_str().unwrap();
    let suggested = message.split_once("Use: bus ").unwrap().1;
    // The selector needs no shell unquoting. Strip the known quoted text
    // placeholder so the parser receives the same argv as a shell would.
    let mut corrected = suggested
        .strip_suffix(" --text '...'")
        .unwrap()
        .split_whitespace()
        .map(String::from)
        .collect::<Vec<_>>();
    corrected.extend(["--text", "..."].map(String::from));
    let parsed = parse(&corrected, "corrected-selector");
    assert!(
        parsed.is_ok(),
        "the suggested command must parse: {suggested}\n{parsed:?}"
    );
    assert_eq!(parsed.unwrap().params["as"], "-worker");
}

#[test]
fn reply_diagnostics_missing_text_never_sends_an_ordinary_message_or_reply() {
    for argv in [
        vec!["send", "--room", "1", "--to", "worker"],
        vec!["send", "--reply-to", "123", "--as", "worker"],
    ] {
        let mut output = Vec::new();
        assert!(run_with(
            &argv.into_iter().map(String::from).collect::<Vec<_>>(),
            &mut output,
            |_, _| panic!("missing text must never reach the server")
        )
        .is_err());
        let response: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(response["ok"], false);
        assert_eq!(response["error"]["code"], "invalid_arguments");
    }
}
