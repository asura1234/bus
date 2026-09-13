use super::*;

#[test]
fn dev_focus_cli_requires_one_target_and_maps_agent_and_room() {
    for (kind, target, method) in [
        ("agent", "42", "agent.focus"),
        ("room", "review", "room.focus"),
    ] {
        let args = [kind, "focus", target].map(str::to_owned);
        let parsed = parse(&args, "focus-cli");
        assert!(parsed.is_ok(), "{parsed:?}");
        let parsed = parsed.unwrap();
        assert_eq!(parsed.method, method);
        assert_eq!(parsed.params[kind], target);
        assert!(parsed.wait_timeout.is_none());
        assert!(parse(&[kind.into(), "focus".into()], "missing").is_err());
        assert!(parse(
            &[kind.into(), "focus".into(), target.into(), "extra".into()],
            "extra"
        )
        .is_err());
    }
}
