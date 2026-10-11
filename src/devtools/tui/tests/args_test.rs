use super::*;

fn parsed(args: &[&str]) -> Parsed {
    Parsed::parse(&args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>()).unwrap()
}

#[test]
fn values_switches_and_positionals_are_split() {
    let mut p = parsed(&["find", "--regex", "--rows", "2-4"]);
    assert_eq!(p.take_value("--rows").as_deref(), Some("2-4"));
    assert!(p.take_switch("--regex"));
    assert!(!p.take_switch("--gone"));
    assert_eq!(p.positional().as_deref(), Some("find"));
    assert!(p.finish().is_ok());
    assert!(Parsed::parse(&["--rows".into()]).is_err());
    let leftover = parsed(&["--bogus", "1"]);
    assert!(leftover.finish().unwrap_err().contains("--bogus"));
}

#[test]
fn sizes_durations_and_rows_parse() {
    assert_eq!(parse_size("160x45").unwrap(), (160, 45));
    assert!(parse_size("10x5").is_err());
    assert!(parse_size("wide").is_err());
    assert_eq!(parse_duration("250ms").unwrap().as_millis(), 250);
    assert_eq!(parse_duration("2s").unwrap().as_secs(), 2);
    assert_eq!(parse_duration("3m").unwrap().as_secs(), 180);
    assert_eq!(parse_duration("1h").unwrap().as_secs(), 3600);
    assert_eq!(parse_duration("7d").unwrap().as_secs(), 7 * 86400);
    assert_eq!(parse_duration("40").unwrap().as_millis(), 40);
    assert!(parse_duration("soon").is_err());
    assert_eq!(parse_rows("3-9").unwrap(), (3, 9));
    assert_eq!(parse_rows("4").unwrap(), (4, 4));
    assert!(parse_rows("9-3").is_err());
}

#[test]
fn verbs_build_host_commands() {
    let click = command(
        "click",
        &mut parsed(&["--text", "Add", "--nth", "2", "--double"]),
    )
    .unwrap();
    assert_eq!(
        click,
        Command::Click {
            target: Target::Text {
                text: "Add".into(),
                regex: false,
                nth: Some(2),
                rows: None
            },
            button: "left".into(),
            mods: String::new(),
            double: true
        }
    );
    assert!(matches!(
        command("click", &mut parsed(&["3", "4"])).unwrap(),
        Command::Click {
            target: Target::Cell { row: 3, col: 4 },
            ..
        }
    ));
    assert!(matches!(
        command("drag", &mut parsed(&["1", "2", "3", "4"])).unwrap(),
        Command::Drag { .. }
    ));
    assert!(command("drag", &mut parsed(&["--from-text", "a"])).is_err());
    assert!(matches!(
        command("drag", &mut parsed(&["--from-text", "a", "--to-text", "b"])).unwrap(),
        Command::Drag { .. }
    ));
    assert!(matches!(
        command("scroll", &mut parsed(&["5", "6", "--down", "3"])).unwrap(),
        Command::Scroll {
            up: false,
            notches: 3,
            ..
        }
    ));
    assert!(command("scroll", &mut parsed(&["5", "6"])).is_err());
    assert!(matches!(
        command("cells", &mut parsed(&["1", "2", "3", "4"])).unwrap(),
        Command::Cells {
            width: 3,
            height: 4,
            ..
        }
    ));
    assert!(matches!(
        command("cells", &mut parsed(&["1", "2"])).unwrap(),
        Command::Cells { width: 1, .. }
    ));
    assert!(command("cells", &mut parsed(&["1"])).is_err());
    assert!(matches!(
        command("press", &mut parsed(&["ctrl+n", "enter"])).unwrap(),
        Command::Press { .. }
    ));
    assert!(command("press", &mut parsed(&[])).is_err());
    assert!(matches!(
        command("resize", &mut parsed(&["100x30"])).unwrap(),
        Command::Resize {
            cols: 100,
            rows: 30
        }
    ));
    for verb in ["status", "clipboard", "snapshot"] {
        assert!(command(verb, &mut parsed(&[])).is_ok());
    }
    assert!(matches!(
        command("type", &mut parsed(&["hi"])).unwrap(),
        Command::Type { .. }
    ));
    assert!(matches!(
        command("paste", &mut parsed(&["hi"])).unwrap(),
        Command::Paste { .. }
    ));
    assert!(matches!(
        command("find", &mut parsed(&["x", "--regex"])).unwrap(),
        Command::Find { regex: true, .. }
    ));
    assert!(command("teleport", &mut parsed(&[])).is_err());
    assert!(command("click", &mut parsed(&["3"])).is_err());
}

#[test]
fn waits_need_a_condition() {
    let timeout = std::time::Duration::from_secs(2);
    assert!(matches!(
        wait_command(&mut parsed(&["--text", "Goal", "--gone"]), timeout).unwrap(),
        Command::WaitText {
            gone: true,
            timeout_ms: 2000,
            ..
        }
    ));
    assert!(matches!(
        wait_command(
            &mut parsed(&["--stable", "300ms", "--rows", "1-2"]),
            timeout
        )
        .unwrap(),
        Command::WaitStable {
            stable_ms: 300,
            rows: Some((1, 2)),
            ..
        }
    ));
    assert!(wait_command(&mut parsed(&[]), timeout).is_err());
}

#[test]
fn help_comes_from_the_doc_sections() {
    let doc = "# Title\nintro\n### start\nstart text\n### stop\nstop text\n";
    assert_eq!(help_overview(doc), "# Title\nintro\n");
    assert_eq!(help_for(doc, "start").unwrap(), "### start\nstart text\n");
    assert!(help_for(doc, "nope").is_none());
    // Every verb the CLI accepts has a section in the shipped doc.
    let shipped = include_str!("../../../../docs/tui-driver.md");
    for verb in [
        "start",
        "stop",
        "status",
        "list",
        "bus",
        "snapshot",
        "find",
        "cells",
        "type",
        "paste",
        "press",
        "click",
        "drag",
        "scroll",
        "resize",
        "wait",
        "expect",
        "clipboard",
        "journey",
        "gc",
    ] {
        assert!(
            help_for(shipped, verb).is_some(),
            "docs/tui-driver.md lacks ### {verb}"
        );
    }
}
