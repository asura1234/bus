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

#[test]
fn an_emoji_grapheme_at_the_wrap_boundary_preserves_the_composer_rule() {
    use crate::devtools::tui::screen::Screen;

    let mut screen = Screen::new(80, 12).unwrap();
    screen.feed(b"transcript sentinel\r\n");
    screen.feed(format!("\r\n{}\r\n", "─".repeat(60)).as_bytes());
    screen.feed(prompt_box("", "claude", 80).as_bytes());
    // The joined emoji occupies two cells in Ghostty's default grapheme mode.
    let draft = format!("{}👩‍💻", "x".repeat(76));
    screen.feed(prompt_box(&draft, "claude", 80).as_bytes());
    screen.feed(prompt_box(&format!("{draft}y"), "claude", 80).as_bytes());
    let visible: Vec<String> = screen
        .grid()
        .cells
        .iter()
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .map(|row| row.trim_end().to_owned())
        .collect();
    let prompt_row = visible.iter().position(|row| row.starts_with('❯')).unwrap();
    assert!(
        prompt_row > 0 && visible[prompt_row - 1].starts_with('─'),
        "the composer reader requires its top rule to survive redraw: {visible:#?}"
    );
    assert!(visible.iter().any(|row| row == "transcript sentinel"));
}

fn screen_rows_of(screen: &crate::devtools::tui::screen::Screen) -> Vec<String> {
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
}

#[test]
fn the_fake_codex_draws_a_codex_composer_that_bus_can_read() {
    use crate::devtools::tui::screen::Screen;

    let mut screen = Screen::new(80, 12).unwrap();
    // Bus's Codex composer reader takes the last `›` line, up to the model line.
    let composers = |screen: &Screen| -> Vec<String> {
        screen_rows_of(screen)
            .into_iter()
            .filter(|row| row.starts_with('›'))
            .collect()
    };
    for line in 0..30 {
        screen.feed(format!("transcript {line}\r\n").as_bytes());
    }
    screen.feed(prompt_top("codex").as_bytes());
    screen.feed(prompt_box("", "codex", 80).as_bytes());
    let rows = screen_rows_of(&screen);
    assert_eq!(
        composers(&screen),
        vec!["› Ask Codex to do anything"],
        "{rows:#?}"
    );
    let composer = rows.iter().position(|row| row.starts_with('›')).unwrap();
    assert_eq!(rows[composer + 1], "");
    assert_eq!(rows[composer + 2], "  gpt-fake · fake codex");
    assert!(!rows.iter().any(|row| row.starts_with('❯')), "{rows:#?}");

    for draft in ["h", "he", "hey"] {
        screen.feed(prompt_box(draft, "codex", 80).as_bytes());
    }
    assert_eq!(
        composers(&screen),
        vec!["› hey"],
        "{:#?}",
        screen_rows_of(&screen)
    );

    screen.feed(submitted("hey").as_bytes());
    assert!(
        composers(&screen).is_empty(),
        "{:#?}",
        screen_rows_of(&screen)
    );
    screen.feed("• fake reply: hey\r\n".as_bytes());
    screen.feed(prompt_top("codex").as_bytes());
    screen.feed(prompt_box("", "codex", 80).as_bytes());
    let rows = screen_rows_of(&screen);
    assert_eq!(
        composers(&screen),
        vec!["› Ask Codex to do anything"],
        "{rows:#?}"
    );
    // One blank row parts the reply from the composer, as in Codex.
    let reply = rows
        .iter()
        .position(|row| row == "• fake reply: hey")
        .unwrap();
    assert_eq!(rows[reply + 1], "", "{rows:#?}");
    assert_eq!(rows[reply + 2], "› Ask Codex to do anything", "{rows:#?}");
    assert!(rows.contains(&"> hey".to_owned()));
}

#[test]
fn the_fake_codex_reads_the_hooks_bus_writes_for_its_project() {
    let project = std::env::temp_dir().join(format!("bus-tui-fake-codex-{}", std::process::id()));
    let nested = project.join("crate/src");
    std::fs::create_dir_all(&nested).unwrap();
    let settings = vec!["--settings".to_owned(), "/x.json".to_owned()];
    assert_eq!(hooks_path("codex", &settings, &nested), None);
    std::fs::create_dir_all(project.join(".codex")).unwrap();
    std::fs::write(project.join(".codex/hooks.json"), "{}").unwrap();
    assert_eq!(
        hooks_path("codex", &[], &nested),
        Some(project.join(".codex/hooks.json"))
    );
    // Claude never looks there: it reads only the settings Bus passes it.
    assert_eq!(hooks_path("claude", &[], &nested), None);
    assert_eq!(
        hooks_path("claude", &settings, &nested),
        Some("/x.json".into())
    );
    std::fs::remove_dir_all(&project).unwrap();
}

#[test]
fn the_fake_codex_hooks_decode_as_an_interactive_codex_turn() {
    use crate::agents::providers::codex::hooks::parse;
    use crate::agents::providers::spool::Parsed;

    let agent = FakeAgent::new("codex", Value::Null);
    let session = agent.session.clone();
    let start = agent.payload("SessionStart", None, json!({"source": "startup"}));
    assert_eq!(
        parse(&start).unwrap(),
        Parsed::Session {
            session: session.clone(),
            source: Some("startup".into())
        }
    );
    let started = agent.payload("UserPromptSubmit", Some("t1"), json!({"prompt": "hey"}));
    assert_eq!(
        parse(&started).unwrap(),
        Parsed::Started {
            session: session.clone(),
            turn: "t1".into(),
            prompt: "hey".into()
        }
    );
    let stop = agent.payload(
        "Stop",
        Some("t1"),
        json!({"last_assistant_message": "fake reply: hey"}),
    );
    assert_eq!(
        parse(&stop).unwrap(),
        Parsed::Final {
            session,
            turn: "t1".into(),
            text: "fake reply: hey".into()
        }
    );
    // No usage is read from it: Bus takes only Codex `rollout-*` files.
    assert_eq!(
        crate::agents::providers::codex::usage::codex_rollout_path(&stop),
        None
    );
    // The fake Claude keeps Claude's turn field.
    let claude = FakeAgent::new("claude", Value::Null);
    let claude_started = claude.payload("UserPromptSubmit", Some("t1"), json!({}));
    assert_eq!(claude_started["prompt_id"], "t1");
    assert!(claude_started.get("transcript_path").is_none());
}

#[test]
fn the_fake_codex_keeps_image_paths_as_text_and_works_like_codex() {
    use crate::agents::manifest::{detect_with_osc, DetectionInput};
    use crate::agents::AgentKind;

    let path_line = "\"/tmp/shot.png\"";
    let codex = FakeAgent::new("codex", Value::Null);
    assert_eq!(codex.lift(path_line), (path_line.to_owned(), 0));
    let claude = FakeAgent::new("claude", Value::Null);
    assert_eq!(claude.lift(path_line).1, 1);

    let screen = format!("> hey\n{}", working_line("codex", 1, 3));
    let detection = detect_with_osc(
        AgentKind::Codex,
        DetectionInput {
            screen: &screen,
            osc_title: "",
            osc_progress: "",
        },
    );
    assert!(detection.visible_working, "{screen}");
    assert_eq!(
        working_line("claude", 0, 3),
        "✻ Working… (3s · esc to interrupt)"
    );
}

#[cfg(unix)]
#[test]
fn the_fake_codex_fires_bus_hook_after_a_project_hook_for_the_same_event() {
    use crate::agents::providers::hook_json::{merge_hooks, HookContract};
    use crate::agents::providers::ProviderKind;
    use std::os::unix::fs::PermissionsExt;

    let dir =
        std::env::temp_dir().join(format!("bus-tui-fake-codex-merged-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let delivered = dir.join("delivered.json");
    // Stands in for the Bus executable named in the hook command.
    let bus = dir.join("bus");
    std::fs::write(
        &bus,
        format!("#!/bin/sh\ncat > '{}'\n", delivered.display()),
    )
    .unwrap();
    std::fs::set_permissions(&bus, std::fs::Permissions::from_mode(0o755)).unwrap();
    // A project that already has its own Codex Stop hook, merged the way Bus writes it.
    let project = json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "true"}]}]}});
    let merged = merge_hooks(
        project,
        HookContract::for_provider(ProviderKind::Codex),
        &bus,
    )
    .unwrap();

    let agent = FakeAgent::new("codex", merged["hooks"].clone());
    agent.hook(
        "Stop",
        Some("t1"),
        json!({"last_assistant_message": "fake reply: hey", "stop_hook_active": false}),
    );
    let fired = delivered.is_file();
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(fired, "Bus's Stop hook never ran: {merged:#}");
}

#[test]
fn a_working_fake_codex_still_shows_a_composer_bus_can_type_into() {
    use crate::devtools::tui::screen::Screen;

    // Real Codex keeps its `›` composer under `• Working (…)` while a turn
    // runs, so Bus can type a steering message into it.
    let mut screen = Screen::new(80, 12).unwrap();
    screen.feed(prompt_top("codex").as_bytes());
    screen.feed(prompt_box("hey fake:work 3", "codex", 80).as_bytes());
    // What the fake draws from Enter while its turn works, typing included.
    screen.feed(submitted("hey fake:work 3").as_bytes());
    screen.feed(working_block(&working_line("codex", 0, 0), "", "codex", false, 80).as_bytes());
    for (frame, shown) in ["s", "st", "steer"].into_iter().enumerate() {
        let line = working_line("codex", frame + 1, 1);
        screen.feed(working_block(&line, shown, "codex", true, 80).as_bytes());
    }
    let rows = screen_rows_of(&screen);
    // Codex's spinner alternates `•` and `◦`; Bus's working rule takes both.
    assert!(
        rows.iter()
            .any(|row| row.starts_with("• Working") || row.starts_with("◦ Working")),
        "{rows:#?}"
    );
    assert!(
        rows.iter().any(|row| row.starts_with('›')),
        "no composer while working: {rows:#?}"
    );
    // Redrawn in place: one working line over one composer holding the draft.
    let working: Vec<_> = rows
        .iter()
        .filter(|row| row.contains("Working ("))
        .collect();
    let composers: Vec<_> = rows.iter().filter(|row| row.starts_with('›')).collect();
    assert_eq!((working.len(), composers.len()), (1, 1), "{rows:#?}");
    assert_eq!(composers[0], "› steer");

    // The turn ends: its block goes, the steered prompt joins the transcript.
    let line = working_line("codex", 4, 3);
    screen.feed(work_done(&line, "codex", 80).as_bytes());
    screen.feed(submitted("steer").as_bytes());
    screen.feed("• fake reply: hey fake:work 3\r\n  steer\r\n".as_bytes());
    screen.feed(prompt_top("codex").as_bytes());
    screen.feed(prompt_box("", "codex", 80).as_bytes());
    let rows = screen_rows_of(&screen);
    assert!(
        !rows.iter().any(|row| row.contains("Working (")),
        "{rows:#?}"
    );
    let sent = rows
        .iter()
        .position(|row| row == "> hey fake:work 3")
        .unwrap();
    assert_eq!(rows[sent + 1], "> steer", "{rows:#?}");
    assert_eq!(rows[sent + 2], "• fake reply: hey fake:work 3", "{rows:#?}");
    let composers: Vec<_> = rows.iter().filter(|row| row.starts_with('›')).collect();
    assert_eq!(composers, vec!["› Ask Codex to do anything"], "{rows:#?}");
}

#[test]
fn the_fake_cursor_draws_a_cursor_composer_that_bus_can_read() {
    use crate::devtools::tui::screen::Screen;

    let mut screen = Screen::new(80, 12).unwrap();
    // Bus's Cursor composer reader takes the last `→` (or `>`) line, up to
    // the `▀` border under it.
    let composers = |screen: &Screen| -> Vec<String> {
        screen_rows_of(screen)
            .into_iter()
            .filter(|row| row.starts_with('→'))
            .collect()
    };
    for line in 0..30 {
        screen.feed(format!("transcript {line}\r\n").as_bytes());
    }
    screen.feed(prompt_top("cursor-agent").as_bytes());
    screen.feed(prompt_box("", "cursor-agent", 80).as_bytes());
    let rows = screen_rows_of(&screen);
    assert_eq!(
        composers(&screen),
        vec!["→ Plan, search, build anything"],
        "{rows:#?}"
    );
    let composer = rows.iter().position(|row| row.starts_with('→')).unwrap();
    assert!(rows[composer - 1].starts_with(" ▄"), "{rows:#?}");
    assert!(rows[composer + 1].starts_with(" ▀"), "{rows:#?}");
    assert!(!rows.iter().any(|row| row.starts_with('❯')), "{rows:#?}");

    for draft in ["h", "he", "hey"] {
        screen.feed(prompt_box(draft, "cursor-agent", 80).as_bytes());
    }
    assert_eq!(composers(&screen), vec!["→ hey"]);

    // While its turn works, Cursor asks for a follow-up under a spinner line
    // that Bus's Cursor manifest reads as working.
    screen.feed(submitted("hey fake:work 3").as_bytes());
    let start = working_line("cursor-agent", 0, 0);
    screen.feed(working_block(&start, "", "cursor-agent", false, 80).as_bytes());
    for (frame, shown) in ["", "s", "steer"].into_iter().enumerate() {
        let line = working_line("cursor-agent", frame + 1, 1);
        screen.feed(working_block(&line, shown, "cursor-agent", true, 80).as_bytes());
    }
    let rows = screen_rows_of(&screen);
    let working: Vec<_> = rows
        .iter()
        .filter(|row| row.contains("Generating ("))
        .collect();
    assert_eq!(
        (working.len(), composers(&screen)),
        (1, vec!["→ steer".to_owned()]),
        "{rows:#?}"
    );
    let detection = crate::agents::manifest::detect_with_osc(
        crate::agents::AgentKind::Cursor,
        crate::agents::manifest::DetectionInput {
            screen: &rows.join("\n"),
            osc_title: "",
            osc_progress: "",
        },
    );
    assert!(detection.visible_working, "{rows:#?}");

    // The turn ends: its block goes, and the idle composer comes back.
    screen.feed(work_done(&working_line("cursor-agent", 4, 3), "cursor-agent", 80).as_bytes());
    screen.feed(submitted("steer").as_bytes());
    screen.feed("⏺ fake reply: hey fake:work 3\r\n  steer\r\n".as_bytes());
    screen.feed(prompt_top("cursor-agent").as_bytes());
    screen.feed(prompt_box("", "cursor-agent", 80).as_bytes());
    let rows = screen_rows_of(&screen);
    assert!(
        !rows.iter().any(|row| row.contains("Generating (")),
        "{rows:#?}"
    );
    let sent = rows
        .iter()
        .position(|row| row == "> hey fake:work 3")
        .unwrap();
    assert_eq!(rows[sent + 1], "> steer", "{rows:#?}");
    assert_eq!(
        composers(&screen),
        vec!["→ Plan, search, build anything"],
        "{rows:#?}"
    );
}

#[test]
fn the_fake_cursor_reads_the_hooks_bus_writes_for_its_project() {
    let project = std::env::temp_dir().join(format!("bus-tui-fake-cursor-{}", std::process::id()));
    let nested = project.join("crate/src");
    std::fs::create_dir_all(&nested).unwrap();
    assert_eq!(hooks_path("cursor-agent", &[], &nested), None);
    std::fs::create_dir_all(project.join(".cursor")).unwrap();
    std::fs::write(project.join(".cursor/hooks.json"), "{}").unwrap();
    assert_eq!(
        hooks_path("cursor-agent", &[], &nested),
        Some(project.join(".cursor/hooks.json"))
    );
    std::fs::remove_dir_all(&project).unwrap();
}

#[test]
fn the_fake_cursor_hooks_decode_as_a_cursor_turn() {
    use crate::agents::providers::cursor::hooks::parse;
    use crate::agents::providers::spool::Parsed;

    let agent = FakeAgent::new("cursor-agent", Value::Null);
    let session = agent.session.clone();
    let start = agent.payload("sessionStart", None, json!({"source": "startup"}));
    assert_eq!(
        parse(&start).unwrap(),
        Parsed::Session {
            session: session.clone(),
            source: Some("startup".into())
        }
    );
    let started = agent.payload("beforeSubmitPrompt", Some("g1"), json!({"prompt": "hey"}));
    assert_eq!(
        parse(&started).unwrap(),
        Parsed::Started {
            session: session.clone(),
            turn: "g1".into(),
            prompt: "hey".into()
        }
    );
    let response = agent.payload(
        "afterAgentResponse",
        Some("g1"),
        json!({"text": "fake reply: hey"}),
    );
    assert_eq!(
        parse(&response).unwrap(),
        Parsed::Response {
            session: session.clone(),
            turn: "g1".into(),
            text: "fake reply: hey".into()
        }
    );
    // With no transcript to wait for, Bus settles on the hook's own text.
    assert_eq!(
        crate::agents::providers::cursor::final_reply::settle_text(&response, false),
        Ok("fake reply: hey".into())
    );
    let stop = agent.payload("stop", Some("g1"), json!({"status": "completed"}));
    assert_eq!(
        parse(&stop).unwrap(),
        Parsed::Completed {
            session,
            turn: "g1".into()
        }
    );
    // Cursor keeps a pasted image path as text, as Codex does.
    let path_line = "\"/tmp/shot.png\"";
    assert_eq!(agent.lift(path_line), (path_line.to_owned(), 0));
}

#[cfg(unix)]
#[test]
fn the_fake_cursor_fires_bus_hooks_from_a_cursor_hooks_file() {
    use crate::agents::providers::hook_json::{merge_hooks, HookContract};
    use crate::agents::providers::ProviderKind;
    use std::os::unix::fs::PermissionsExt;

    let dir = std::env::temp_dir().join(format!("bus-tui-fake-cursor-hook-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let delivered = dir.join("delivered.json");
    // Stands in for the Bus executable named in the hook command.
    let bus = dir.join("bus");
    std::fs::write(
        &bus,
        format!("#!/bin/sh\ncat > '{}'\n", delivered.display()),
    )
    .unwrap();
    std::fs::set_permissions(&bus, std::fs::Permissions::from_mode(0o755)).unwrap();
    // Cursor's hooks file as Bus writes it: each entry is the command itself.
    let merged = merge_hooks(
        json!({}),
        HookContract::for_provider(ProviderKind::Cursor),
        &bus,
    )
    .unwrap();

    let agent = FakeAgent::new("cursor-agent", merged["hooks"].clone());
    agent.session_start();
    let fired = std::fs::read_to_string(&delivered).ok();
    std::fs::remove_dir_all(&dir).unwrap();
    let fired: Value =
        serde_json::from_str(&fired.expect("Bus's sessionStart hook never ran")).unwrap();
    assert_eq!(fired["hook_event_name"], "sessionStart");
    assert_eq!(fired["conversation_id"], json!(agent.session));
}

#[test]
fn a_fake_cursor_in_a_narrow_pane_reads_idle_once_its_turn_ends() {
    use crate::devtools::tui::screen::Screen;

    // Narrower than the composer's 60-cell borders, so they wrap.
    let cols = 50;
    let mut screen = Screen::new(cols, 30).unwrap();
    screen.feed(prompt_top("cursor-agent").as_bytes());
    screen.feed(prompt_box("hey fake:work 3", "cursor-agent", cols).as_bytes());
    screen.feed(submitted("hey fake:work 3").as_bytes());
    let start = working_line("cursor-agent", 0, 0);
    screen.feed(working_block(&start, "", "cursor-agent", false, cols).as_bytes());
    for frame in 1..4 {
        let line = working_line("cursor-agent", frame, 1);
        screen.feed(working_block(&line, "", "cursor-agent", true, cols).as_bytes());
    }
    let working = |screen: &Screen| {
        screen_rows_of(screen)
            .iter()
            .filter(|row| row.contains("Generating ("))
            .count()
    };
    assert_eq!(working(&screen), 1, "{:#?}", screen_rows_of(&screen));

    // The turn ends: no working line may outlive it, or Bus reads the idle
    // fake as still working.
    screen.feed(work_done(&working_line("cursor-agent", 4, 3), "cursor-agent", cols).as_bytes());
    screen.feed("⏺ fake reply: hey fake:work 3\r\n".as_bytes());
    screen.feed(prompt_top("cursor-agent").as_bytes());
    screen.feed(prompt_box("", "cursor-agent", cols).as_bytes());
    let rows = screen_rows_of(&screen);
    let detection = crate::agents::manifest::detect_with_osc(
        crate::agents::AgentKind::Cursor,
        crate::agents::manifest::DetectionInput {
            screen: &rows.join("\n"),
            osc_title: "",
            osc_progress: "",
        },
    );
    assert!(!detection.visible_working, "{rows:#?}");
    assert_eq!(working(&screen), 0, "{rows:#?}");
}

#[test]
fn a_fake_cursor_turn_clears_its_working_line_at_every_pane_width() {
    use crate::devtools::tui::screen::Screen;

    // Around the 61-cell borders: narrower, exactly as wide, and wider.
    for cols in 40..=80 {
        let mut screen = Screen::new(cols, 30).unwrap();
        screen.feed(prompt_top("cursor-agent").as_bytes());
        screen.feed(prompt_box("hey fake:work 3", "cursor-agent", cols).as_bytes());
        screen.feed(submitted("hey fake:work 3").as_bytes());
        let start = working_line("cursor-agent", 0, 0);
        screen.feed(working_block(&start, "", "cursor-agent", false, cols).as_bytes());
        for frame in 1..4 {
            let line = working_line("cursor-agent", frame, 1);
            screen.feed(working_block(&line, "steer", "cursor-agent", true, cols).as_bytes());
        }
        let rows = screen_rows_of(&screen);
        let working = rows
            .iter()
            .filter(|row| row.contains("Generating ("))
            .count();
        assert_eq!(working, 1, "{cols} cols: {rows:#?}");
        screen
            .feed(work_done(&working_line("cursor-agent", 4, 3), "cursor-agent", cols).as_bytes());
        let rows = screen_rows_of(&screen);
        assert!(
            !rows
                .iter()
                .any(|row| row.contains("Generating (") || row.starts_with('→')),
            "{cols} cols: {rows:#?}"
        );
        assert_eq!(
            rows.iter().rev().find(|row| !row.is_empty()).unwrap(),
            "> hey fake:work 3",
            "{cols} cols: {rows:#?}"
        );
    }
}
