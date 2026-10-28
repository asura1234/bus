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
            error.contains("bus send --reply-to 28089 --as 26457 --text '...'"),
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
}
