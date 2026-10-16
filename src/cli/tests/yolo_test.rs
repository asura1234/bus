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
