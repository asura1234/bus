use super::*;

#[test]
fn input_splits_text_keys_and_pastes() {
    let mut pending = b"ab\x1b[200~line\nmore\x1b[201~\x7f\r\x1b[13u\x1b[A\x03".to_vec();
    assert_eq!(
        drain_input(&mut pending),
        vec![
            Input::Text("abline\nmore".into()),
            Input::Backspace,
            Input::Enter,
            Input::Enter,
            Input::Interrupt,
        ]
    );
    assert!(pending.is_empty());
}

#[test]
fn unfinished_sequences_wait_for_more_bytes() {
    let mut pending = b"x\x1b[200~half".to_vec();
    assert_eq!(drain_input(&mut pending), vec![Input::Text("x".into())]);
    assert_eq!(pending, b"\x1b[200~half");
    let mut lone = b"\x1b".to_vec();
    assert!(drain_input(&mut lone).is_empty());
    let mut alt = b"\x1bb\x01z".to_vec();
    assert_eq!(drain_input(&mut alt), vec![Input::Text("z".into())]);
}

#[test]
fn work_prompts_and_provider_names() {
    assert_eq!(work_seconds("please fake:work 3 now"), Some(3));
    assert_eq!(work_seconds("fake:work 9999"), Some(600));
    assert_eq!(work_seconds("just hello"), None);
    assert!(PROVIDER_NAMES.contains(&"claude"));
    assert_eq!(
        settings_path(&["--settings".into(), "/x.json".into()]),
        Some("/x.json".into())
    );
    assert_eq!(settings_path(&[]), None);
    assert_eq!(pseudo_uuid().len(), 36);
}

#[test]
fn install_links_every_provider_name() {
    let dir = std::env::temp_dir().join(format!("bus-tui-fake-{}", std::process::id()));
    let binary = std::env::current_exe().unwrap();
    install(&dir, &binary).unwrap();
    for name in PROVIDER_NAMES {
        assert!(dir.join(exe_name(name)).exists(), "{name} missing");
    }
    // Installing again replaces the links.
    install(&dir, &binary).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_character_split_across_reads_is_not_garbled() {
    let mut pending = "你".as_bytes()[..1].to_vec();
    let mut events = drain_input(&mut pending);
    assert!(events.is_empty());
    pending.extend_from_slice(&"你".as_bytes()[1..]);
    events.extend(drain_input(&mut pending));
    assert_eq!(events, vec![Input::Text("你".into())]);
    assert!(pending.is_empty());
}

#[test]
fn image_lines_become_leading_chips_like_claude_code() {
    let draft = "please look\n\"/tmp/a b/shot.png\"\n\"/tmp/notes.txt\"";
    assert_eq!(
        lift_images(draft, 2),
        ("[Image #3]please look\n\"/tmp/notes.txt\"".to_owned(), 1)
    );
    assert_eq!(lift_images("plain\ntext", 0), ("plain\ntext".to_owned(), 0));
}

#[test]
fn repeated_kills_clear_a_multi_line_draft() {
    let mut draft = "one\ntwo".to_owned();
    for expected in ["one\n", "one", ""] {
        kill_line(&mut draft);
        assert_eq!(draft, expected);
    }
    let mut pending = b"a\x0bb\x15\n".to_vec();
    assert_eq!(
        drain_input(&mut pending),
        vec![
            Input::Text("a".into()),
            Input::KillLine,
            Input::Text("b".into()),
            Input::KillLine,
            Input::Text("\n".into()),
        ]
    );
}

#[test]
fn a_full_pane_keeps_one_prompt_box_while_typing_and_after_submit() {
    use crate::devtools::tui::screen::Screen;

    let mut screen = Screen::new(80, 12).unwrap();
    let rows = |screen: &Screen| -> Vec<String> {
        screen
            .grid()
            .cells
            .iter()
            .map(|row| {
                row.iter()
                    .map(|cell| cell.symbol.as_str())
                    .collect::<String>()
            })
            .map(|row| row.trim_end().to_owned())
            .collect()
    };
    let boxes = |screen: &Screen| -> Vec<String> {
        rows(screen)
            .into_iter()
            .filter(|row| row.starts_with('❯'))
            .collect()
    };
    let open_box = format!("\r\n{}\r\n", "─".repeat(60));
    for line in 0..30 {
        screen.feed(format!("transcript {line}\r\n").as_bytes());
    }
    screen.feed(open_box.as_bytes());
    for draft in ["", "h", "he", "hey"] {
        screen.feed(prompt_box(draft, "claude", 80).as_bytes());
    }
    assert_eq!(boxes(&screen), vec!["❯ hey"], "{:#?}", rows(&screen));

    screen.feed(submitted("hey").as_bytes());
    assert!(boxes(&screen).is_empty(), "{:#?}", rows(&screen));
    assert!(rows(&screen).contains(&"> hey".to_owned()));

    screen.feed(b"\xe2\x8f\xba fake reply: hey\r\n");
    screen.feed(open_box.as_bytes());
    screen.feed(prompt_box("", "claude", 80).as_bytes());
    assert_eq!(boxes(&screen), vec!["❯"], "{:#?}", rows(&screen));
}

#[test]
fn the_fake_lifts_exactly_the_lines_bus_expects_claude_to_lift() {
    // An unescaped inner quote makes the line two quoted strings, not one path.
    let draft = "\"/tmp/a\" and \"/tmp/b.png\"";
    assert_eq!(lift_images(draft, 0), (draft.to_owned(), 0));
    let escaped = "\"/tmp/a \\\"b\\\".png\"";
    assert_eq!(lift_images(escaped, 0), ("[Image #1]".to_owned(), 1));
}

#[test]
fn a_wrapped_draft_keeps_one_prompt_box_and_is_cleared_on_submit() {
    use crate::devtools::tui::screen::Screen;

    let mut screen = Screen::new(80, 12).unwrap();
    for line in 0..30 {
        screen.feed(format!("transcript {line}\r\n").as_bytes());
    }
    screen.feed(format!("\r\n{}\r\n", "─".repeat(60)).as_bytes());
    screen.feed(prompt_box("", "claude", 80).as_bytes());
    let draft = "x".repeat(100);
    screen.feed(prompt_box(&draft, "claude", 80).as_bytes());
    screen.feed(prompt_box(&format!("{draft}y"), "claude", 80).as_bytes());
    let rows = |screen: &Screen| -> Vec<String> {
        screen
            .grid()
            .cells
            .iter()
            .map(|row| {
                row.iter()
                    .map(|cell| cell.symbol.as_str())
                    .collect::<String>()
            })
            .map(|row| row.trim_end().to_owned())
            .collect()
    };
    let visible = rows(&screen);
    assert_eq!(
        visible.iter().filter(|row| row.starts_with('❯')).count(),
        1,
        "a draft wider than the pane must still have one composer: {visible:#?}"
    );
    screen.feed(submitted(&format!("{draft}y")).as_bytes());
    let visible = rows(&screen);
    assert!(
        visible.iter().all(|row| !row.starts_with('❯')),
        "submitted text must not remain in a stale composer: {visible:#?}"
    );
}

#[test]
fn a_draft_wider_than_the_pane_leaves_no_stale_prompt_box() {
    use crate::devtools::tui::screen::Screen;

    let mut screen = Screen::new(80, 24).unwrap();
    let rows = |screen: &Screen| -> Vec<String> {
        screen
            .grid()
            .cells
            .iter()
            .map(|row| {
                row.iter()
                    .map(|cell| cell.symbol.as_str())
                    .collect::<String>()
            })
            .map(|row| row.trim_end().to_owned())
            .collect()
    };
    // A room message as Bus types it into the composer, wider than the pane.
    let message = "[room general] alice: please review the attached plan and reply with the risks you see first!!!";
    assert_eq!(message.chars().count(), 95);
    screen.feed(format!("\r\n{}\r\n", "─".repeat(60)).as_bytes());
    for typed in 0..=message.len() {
        screen.feed(prompt_box(&message[..typed], "claude", 80).as_bytes());
    }
    let prompts: Vec<String> = rows(&screen)
        .into_iter()
        .filter(|row| row.starts_with('❯'))
        .collect();
    assert_eq!(prompts, vec![format!("❯ {}", &message[..78])]);

    screen.feed(submitted(message).as_bytes());
    let after = rows(&screen);
    assert!(
        after.iter().all(|row| !row.starts_with('❯')),
        "after submit: {after:#?}"
    );
    assert!(
        after.contains(&format!("> {}", &message[..78])),
        "{after:#?}"
    );
}

#[test]
fn wide_character_wraps_keep_one_prompt_box_in_an_odd_width_pane() {
    use crate::devtools::tui::screen::Screen;

    let mut screen = Screen::new(79, 12).unwrap();
    for line in 0..30 {
        screen.feed(format!("transcript {line}\r\n").as_bytes());
    }
    screen.feed(format!("\r\n{}\r\n", "─".repeat(60)).as_bytes());
    screen.feed(prompt_box("", "claude", 79).as_bytes());
    // A wide character cannot occupy the single cell left at a row's end.
    let draft = "界".repeat(156);
    screen.feed(prompt_box(&draft, "claude", 79).as_bytes());
    screen.feed(prompt_box(&format!("{draft}y"), "claude", 79).as_bytes());
    let rows = |screen: &Screen| -> Vec<String> {
        screen
            .grid()
            .cells
            .iter()
            .map(|row| {
                row.iter()
                    .map(|cell| cell.symbol.as_str())
                    .collect::<String>()
            })
            .map(|row| row.trim_end().to_owned())
            .collect()
    };
    let visible = rows(&screen);
    assert_eq!(
        visible.iter().filter(|row| row.starts_with('❯')).count(),
        1,
        "wide-character wrapping must keep one composer: {visible:#?}"
    );
    screen.feed(submitted(&format!("{draft}y")).as_bytes());
    let visible = rows(&screen);
    assert!(
        visible.iter().all(|row| !row.starts_with('❯')),
        "submitted text must not remain in a stale composer: {visible:#?}"
    );
}

#[test]
fn a_wrapped_draft_with_wide_characters_leaves_no_stale_prompt_box() {
    use crate::devtools::tui::screen::Screen;

    let mut screen = Screen::new(80, 24).unwrap();
    let rows = |screen: &Screen| -> Vec<String> {
        screen
            .grid()
            .cells
            .iter()
            .map(|row| {
                row.iter()
                    .map(|cell| cell.symbol.as_str())
                    .collect::<String>()
            })
            .map(|row| row.trim_end().to_owned())
            .collect()
    };
    // Width 160, but the 39th `审` would start in column 79 and wraps early,
    // so the draft takes three rows, not two.
    let draft = format!("a{}{}", "审".repeat(39), "x".repeat(79));
    assert_eq!(screen_rows(&format!("❯ {draft}"), 80), 3);
    screen.feed(format!("\r\n{}\r\n", "─".repeat(60)).as_bytes());
    screen.feed(prompt_box("", "claude", 80).as_bytes());
    screen.feed(prompt_box(&draft, "claude", 80).as_bytes());
    let prompts = rows(&screen)
        .into_iter()
        .filter(|row| row.starts_with('❯'))
        .count();
    assert_eq!(prompts, 1, "{:#?}", rows(&screen));
    screen.feed(submitted(&draft).as_bytes());
    let after = rows(&screen);
    assert!(
        after.iter().all(|row| !row.starts_with('❯')),
        "after submit: {after:#?}"
    );
}
