use super::*;
use crate::messaging::model::{Draft, Provider};
use std::time::Instant;

#[cfg(unix)]
fn thread_cpu_us() -> u64 {
    let mut value = std::mem::MaybeUninit::<libc::timespec>::uninit();
    assert_eq!(
        unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, value.as_mut_ptr()) },
        0
    );
    let value = unsafe { value.assume_init() };
    value.tv_sec as u64 * 1_000_000 + value.tv_nsec as u64 / 1_000
}

#[cfg(not(unix))]
fn thread_cpu_us() -> u64 {
    0
}

fn large_room() -> (BusState, RoomId) {
    let mut state = BusState::default();
    let room = state.create_room("large").unwrap();
    let agent = state
        .create_agent(room, "worker", Provider::Codex, "/project".into(), None)
        .unwrap();
    for index in 0..2500 {
        let id = state.submit_message_from(room, Draft {
            text: format!("**Prompt {index}**\nRead src/client/rooms/history/exchange.rs and https://example.com/long/path/to/documentation\nKeep output identical."),
            files: vec![], recipient_ids: [agent].into(),
        }, Author::Developer, 1_800_000_000_000 + index).unwrap()[0];
        state.finish_for_test(id, "## Result\n\n- **First** implementation detail\n- Second detail with `code`\n- Third detail\n\nSee https://example.com/long/path/to/documentation and src/client/rooms/history/exchange.rs.\n\n```rust\nlet answer = 42;\n```", 1_800_000_003_000 + index);
    }
    (state, room)
}

fn phases(label: &str) {
    use super::super::phases::*;
    let samples: Vec<_> = [
        ("signature", &SIGNATURE),
        ("grouping", &GROUPING),
        ("parse", &PARSE),
        ("layout", &LAYOUT),
        ("prepare", &PREPARE),
        ("paint", &PAINT),
        ("rows", &ROWS),
        ("links", &LINKS),
        ("headers", &HEADER),
        ("wrap", &WRAP),
        ("thumbnails", &THUMBNAILS),
    ]
    .into_iter()
    .filter_map(|(name, timer)| timer.take().map(|w| (name, w.count, w.total_us)))
    .collect();
    eprintln!("{label}: phases (name, count, us)={samples:?}");
}

#[test]
#[ignore = "manual phase timing; run alone with --nocapture --ignored"]
fn large_room_phase_profile() {
    let (mut state, room) = large_room();
    let other = state.create_room("other").unwrap();
    let mut history = History::default();
    let mut thumbnails = Thumbnails::default();
    for (label, id, width, now) in [
        ("cold", room, 100, 1_800_000_010_000),
        ("minute", room, 100, 1_800_000_070_000),
        ("other", other, 100, 1_800_000_070_000),
        ("return", room, 100, 1_800_000_070_000),
        ("resize", room, 80, 1_800_000_070_000),
    ] {
        let start = Instant::now();
        let cpu = thread_cpu_us();
        let rows = history
            .lines(
                &state,
                state.room(id).unwrap(),
                width,
                1,
                now,
                &mut thumbnails,
            )
            .len();
        eprintln!(
            "{label}: rows={rows} rendered={} elapsed_us={} thread_cpu_us={}",
            history.rendered,
            start.elapsed().as_micros(),
            thread_cpu_us() - cpu,
        );
        phases(label);
    }
}

#[test]
fn large_room_compose_does_not_rebuild_exchanges() {
    let (state, room) = large_room();
    let snapshot = Arc::new(crate::messaging::coordinator::BusSnapshot {
        state,
        revision: 1,
        last_command_id: 0,
        error: None,
    });
    let mut ui = crate::client::rooms::BusUi::new(snapshot);
    ui.room = Some(room);
    ui.history.enable_background().unwrap();
    ui.compute_view(124, 40);
    assert_eq!(
        ui.history.rebuilt, 0,
        "compose must not rebuild the whole room"
    );
    let mut thumbnails = Thumbnails::default();
    let width = ui.view.history_text.width;
    let snap = Arc::clone(&ui.snapshot);
    await_layout(
        &mut ui.history,
        Arc::clone(&snap),
        room,
        width,
        1_800_000_010_000,
        &mut thumbnails,
    );
    assert!(ui.history.cached().len() >= 35_000);
    let original = Arc::clone(&ui.history.lines);
    ui.history.request_lines(
        Arc::clone(&snap),
        room,
        width,
        1_800_000_070_000,
        &mut thumbnails,
    );
    assert!(
        Arc::ptr_eq(&original, &ui.history.lines),
        "minute reuses the full layout"
    );
    ui.compute_view(100, 40);
    assert_eq!(
        ui.history.rebuilt, 0,
        "resize must enqueue layout without rebuilding in compose"
    );
    let width = ui.view.history_text.width;
    await_layout(
        &mut ui.history,
        snap,
        room,
        width,
        1_800_000_070_000,
        &mut thumbnails,
    );
    assert!(ui.history.cached().len() >= 35_000);
}

fn snapshot(state: BusState, revision: u64) -> Arc<crate::messaging::coordinator::BusSnapshot> {
    Arc::new(crate::messaging::coordinator::BusSnapshot {
        state,
        revision,
        last_command_id: 0,
        error: None,
    })
}

fn await_layout(
    history: &mut History,
    snapshot: Arc<crate::messaging::coordinator::BusSnapshot>,
    room: RoomId,
    width: u16,
    now: u64,
    thumbnails: &mut Thumbnails,
) {
    let deadline = Instant::now() + std::time::Duration::from_secs(30);
    loop {
        history.request_lines(Arc::clone(&snapshot), room, width, now, thumbnails);
        if history.background.as_ref().unwrap().is_current(
            snapshot.revision,
            room,
            width,
            now,
            thumbnails.layout_key(),
        ) {
            return;
        }
        assert!(Instant::now() < deadline, "history worker did not finish");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[test]
fn background_layout_matches_every_row_of_the_synchronous_renderer() {
    let mut state = BusState::default();
    let first = state.create_room("first").unwrap();
    let second = state.create_room("second").unwrap();
    let mut ids = Vec::new();
    for room in [first, second] {
        let agent = state
            .create_agent(
                room,
                &format!("worker-{}", room.0),
                Provider::Codex,
                "/project".into(),
                None,
            )
            .unwrap();
        let id = state.submit_message_from(room, Draft {
            text: "**Hello** 中文 👩‍💻\nA long https://example.com/a/b/c/d/e/f/g/h/i/j/k link\n[waiting-on-you]".into(),
            files: vec!["/missing/notes.md".into()], recipient_ids: [agent].into(),
        }, Author::Developer, 1_800_000_000_000).unwrap()[0];
        state.finish_for_test(id, "# Result\n\n> quoted **text**\n\n- one\n- two\n\n| a | b |\n|---|---|\n| alpha | beta |\n\n```rust\nlet n = 1;\n```\n\n[docs](https://example.com)", 1_800_000_001_000);
        ids.push(agent);
    }
    let mut history = History::default();
    history.enable_background().unwrap();
    let mut thumbnails = Thumbnails::default();
    for (revision, room, width, now) in [
        (1, first, 80, 1_800_000_010_000),
        (1, second, 80, 1_800_000_010_000),
        (1, first, 80, 1_800_000_070_000),
        (1, first, 19, 1_800_000_070_000),
        (2, first, 19, 1_800_000_130_000),
        (2, first, 19, 1_800_100_000_000),
        (2, first, 19, 1_800_000_010_000),
    ] {
        if revision == 2 {
            state.rename_agent(ids[0], "renamed").unwrap();
        }
        let snapshot = snapshot(state.clone(), revision);
        await_layout(
            &mut history,
            Arc::clone(&snapshot),
            room,
            width,
            now,
            &mut thumbnails,
        );
        let mut reference = History::default();
        let expected = reference.lines(
            &snapshot.state,
            snapshot.state.room(room).unwrap(),
            width,
            revision,
            now,
            &mut Thumbnails::default(),
        );
        assert_eq!(
            history.cached(),
            expected,
            "revision={revision} room={room:?} width={width} now={now}"
        );
        assert_eq!(history.rebuilt, 0);
    }
}

#[test]
fn minute_changes_reuse_rows_until_an_age_label_actually_changes() {
    let mut state = BusState::default();
    let room = state.create_room("ages").unwrap();
    let agent = state
        .create_agent(room, "worker", Provider::Codex, "/project".into(), None)
        .unwrap();
    state
        .submit_message_from(
            room,
            Draft {
                text: "**old prompt**".into(),
                files: vec![],
                recipient_ids: [agent].into(),
            },
            Author::Developer,
            1_800_000_000_000,
        )
        .unwrap();
    let mut history = History::default();
    let mut thumbnails = Thumbnails::default();
    for (now, rebuilt) in [
        (1_800_000_010_000, 1),
        (1_800_000_070_000, 0),
        (1_800_086_400_000, 0),
        (1_800_086_460_000, 1),
        (1_800_086_520_000, 0),
        (1_800_000_010_000, 1),
    ] {
        history.lines(
            &state,
            state.room(room).unwrap(),
            80,
            1,
            now,
            &mut thumbnails,
        );
        assert_eq!(history.rebuilt, rebuilt, "now={now}");
        let mut reference = History::default();
        assert_eq!(
            history.cached(),
            reference.lines(
                &state,
                state.room(room).unwrap(),
                80,
                1,
                now,
                &mut Thumbnails::default()
            )
        );
    }
}

#[test]
fn coalesced_room_and_width_changes_only_publish_the_latest_request() {
    let (mut state, large) = large_room();
    let small = state.create_room("small").unwrap();
    let agent = state
        .create_agent(
            small,
            "small-worker",
            Provider::Codex,
            "/project".into(),
            None,
        )
        .unwrap();
    state
        .submit_message_from(
            small,
            Draft {
                text: "latest room".into(),
                files: vec![],
                recipient_ids: [agent].into(),
            },
            Author::Developer,
            1_800_000_000_000,
        )
        .unwrap();
    let snapshot = snapshot(state, 1);
    let mut history = History::default();
    history.enable_background().unwrap();
    let mut thumbnails = Thumbnails::default();
    history.request_lines(
        Arc::clone(&snapshot),
        large,
        100,
        1_800_000_010_000,
        &mut thumbnails,
    );
    for width in 20..100 {
        history.request_lines(
            Arc::clone(&snapshot),
            small,
            width,
            1_800_000_010_000,
            &mut thumbnails,
        );
        assert_eq!(history.rebuilt, 0);
    }
    await_layout(
        &mut history,
        Arc::clone(&snapshot),
        small,
        60,
        1_800_000_010_000,
        &mut thumbnails,
    );
    let mut reference = History::default();
    assert_eq!(
        history.cached(),
        reference.lines(
            &snapshot.state,
            snapshot.state.room(small).unwrap(),
            60,
            1,
            1_800_000_010_000,
            &mut Thumbnails::default()
        )
    );
    assert!(history
        .cached()
        .iter()
        .any(|line| line.text == "latest room"));
}

#[test]
#[ignore = "manual interaction timing; run alone with --nocapture --ignored"]
fn background_interaction_profile() {
    let (mut state, room) = large_room();
    let other = state.create_room("other").unwrap();
    let snapshot = snapshot(state, 1);
    let mut history = History::default();
    history.enable_background().unwrap();
    let mut thumbnails = Thumbnails::default();
    for (label, id, width, now) in [
        ("cold", room, 100, 1_800_000_010_000),
        ("minute", room, 100, 1_800_000_070_000),
        ("other", other, 100, 1_800_000_070_000),
        ("return", room, 100, 1_800_000_070_000),
        ("resize", room, 80, 1_800_000_070_000),
    ] {
        let start = Instant::now();
        history.request_lines(Arc::clone(&snapshot), id, width, now, &mut thumbnails);
        let request_us = start.elapsed().as_micros();
        await_layout(
            &mut history,
            Arc::clone(&snapshot),
            id,
            width,
            now,
            &mut thumbnails,
        );
        eprintln!(
            "{label}: request_us={request_us} completion_us={} rows={} compose_exchanges={}",
            start.elapsed().as_micros(),
            history.cached().len(),
            history.rebuilt
        );
        phases(label);
    }
}

#[test]
fn snapshot_revisions_do_not_starve_a_cold_history_layout() {
    let (state, room) = large_room();
    let mut history = History::default();
    history.enable_background().unwrap();
    let mut thumbnails = Thumbnails::default();
    let deadline = Instant::now() + std::time::Duration::from_secs(30);
    let mut revision = 0;
    while history.cached().is_empty() {
        revision += 1;
        history.request_lines(
            snapshot(state.clone(), revision),
            room,
            100,
            1_800_000_010_000,
            &mut thumbnails,
        );
        assert_eq!(history.rebuilt, 0);
        assert!(
            Instant::now() < deadline,
            "poll revisions starved the initial layout"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(history.cached().len() >= 35_000);
}

#[test]
#[ignore = "paired full compute_view timing; run alone with --nocapture --ignored"]
fn large_room_ui_profile() {
    let (mut state, room) = large_room();
    let other = state.create_room("other").unwrap();
    let snapshot = snapshot(state, 1);
    for background in [false, true] {
        let mut ui = crate::client::rooms::BusUi::new(Arc::clone(&snapshot));
        if background {
            ui.history.enable_background().unwrap();
        }
        for (label, id, cols) in [
            ("cold", room, 124),
            ("resize", room, 100),
            ("other", other, 100),
            ("return", room, 100),
        ] {
            ui.room = Some(id);
            let start = Instant::now();
            let cpu = thread_cpu_us();
            ui.compute_view(cols, 40);
            eprintln!(
                "UI background={background} {label}: frame_us={} thread_cpu_us={} exchanges={}",
                start.elapsed().as_micros(),
                thread_cpu_us() - cpu,
                ui.history.rebuilt
            );
            if background {
                let deadline = Instant::now() + std::time::Duration::from_secs(30);
                let mut max_frame_us = 0;
                loop {
                    let start = Instant::now();
                    ui.compute_view(cols, 40);
                    max_frame_us = max_frame_us.max(start.elapsed().as_micros());
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as u64;
                    if ui.history.background.as_ref().unwrap().is_current(
                        snapshot.revision,
                        id,
                        ui.view.history_text.width,
                        now,
                        ui.thumbnails.layout_key(),
                    ) {
                        break;
                    }
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                eprintln!(
                    "UI background={background} {label}: max_pending_or_apply_frame_us={max_frame_us} rows={}",
                    ui.history.cached().len()
                );
            }
        }
    }
}

#[test]
fn background_thumbnails_match_and_do_not_overflow_during_resize() {
    let dir = std::env::temp_dir().join(format!(
        "bus-history-image-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("attachment.png");
    image::RgbaImage::new(480, 80).save(&file).unwrap();
    let mut state = BusState::default();
    let room = state.create_room("images").unwrap();
    let agent = state
        .create_agent(room, "worker", Provider::Codex, "/project".into(), None)
        .unwrap();
    state
        .submit_message_from(
            room,
            Draft {
                text: "picture".into(),
                files: vec![file],
                recipient_ids: [agent].into(),
            },
            Author::Developer,
            1_800_000_000_000,
        )
        .unwrap();
    let snapshot = snapshot(state, 1);
    let mut ui = crate::client::rooms::BusUi::new(Arc::clone(&snapshot));
    ui.history.enable_background().unwrap();
    let cell = crate::protocol::kitty::HostCellSize {
        width_px: 10,
        height_px: 20,
    };
    ui.thumbnails.set_cell(Some(cell));
    ui.compute_view(124, 40);
    let width = ui.view.history_text.width;
    await_layout(
        &mut ui.history,
        Arc::clone(&snapshot),
        room,
        width,
        1_800_000_010_000,
        &mut ui.thumbnails,
    );
    ui.compute_view(124, 40);
    assert_eq!(ui.view.thumbnails.len(), 1);
    let cached_cols = ui.view.thumbnails[0].cols;
    let mut reference = History::default();
    let mut thumbnails = Thumbnails::default();
    thumbnails.set_cell(Some(cell));
    assert_eq!(
        ui.history.cached(),
        reference.lines(
            &snapshot.state,
            snapshot.state.room(room).unwrap(),
            width,
            1,
            1_800_000_010_000,
            &mut thumbnails
        )
    );
    ui.compute_view(40, 40);
    assert!(
        ui.view.history_text.width < cached_cols,
        "exercise a narrower viewport than the cached image"
    );
    assert!(ui.view.thumbnails.is_empty());
    assert_eq!(ui.history.rebuilt, 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn append_only_projects_links_for_the_new_message() {
    let mut state = BusState::default();
    let room = state.create_room("links").unwrap();
    let agent = state
        .create_agent(room, "worker", Provider::Codex, "/project".into(), None)
        .unwrap();
    let draft = || Draft {
        text: "https://example.com/a/b/c/d/e/f/g/h/i/j/k/l/m".into(),
        files: vec![],
        recipient_ids: [agent].into(),
    };
    let request = state
        .submit_message_from(room, draft(), Author::Developer, 1_800_000_000_000)
        .unwrap()[0];
    state.finish_for_test(
        request,
        "Read src/client/rooms/history/exchange.rs",
        1_800_000_001_000,
    );
    let mut history = History::default();
    let mut thumbnails = Thumbnails::default();
    history.lines(
        &state,
        state.room(room).unwrap(),
        30,
        1,
        1_800_000_010_000,
        &mut thumbnails,
    );
    assert_eq!(history.projected, 2);
    state
        .submit_message_from(room, draft(), Author::Developer, 1_800_000_002_000)
        .unwrap();
    history.lines(
        &state,
        state.room(room).unwrap(),
        30,
        2,
        1_800_000_010_000,
        &mut thumbnails,
    );
    assert_eq!(
        history.projected, 1,
        "unchanged Markdown keeps its source-to-row link projection"
    );
    let mut reference = History::default();
    assert_eq!(
        history.cached(),
        reference.lines(
            &state,
            state.room(room).unwrap(),
            30,
            2,
            1_800_000_010_000,
            &mut Thumbnails::default()
        )
    );
}

fn rooms_with_history(count: usize) -> (BusState, Vec<RoomId>) {
    let mut state = BusState::default();
    let mut rooms = Vec::new();
    for index in 0..count {
        let room = state.create_room(&format!("room-{index}")).unwrap();
        let agent = state
            .create_agent(
                room,
                &format!("worker-{index}"),
                Provider::Codex,
                "/project".into(),
                None,
            )
            .unwrap();
        state
            .submit_message_from(
                room,
                Draft {
                    text: format!("visited room {index}"),
                    files: vec![],
                    recipient_ids: [agent].into(),
                },
                Author::Developer,
                1_800_000_000_000,
            )
            .unwrap();
        rooms.push(room);
    }
    (state, rooms)
}

#[test]
fn returning_to_a_fifth_visited_room_keeps_its_history_visible() {
    let (state, rooms) = rooms_with_history(5);
    let snapshot = snapshot(state, 1);
    let mut history = History::default();
    history.enable_background().unwrap();
    let mut thumbnails = Thumbnails::default();
    for room in &rooms {
        await_layout(
            &mut history,
            Arc::clone(&snapshot),
            *room,
            80,
            1_800_000_010_000,
            &mut thumbnails,
        );
    }
    history.request_lines(
        Arc::clone(&snapshot),
        rooms[0],
        80,
        1_800_000_010_000,
        &mut thumbnails,
    );
    assert!(
        history
            .cached()
            .iter()
            .any(|line| line.text.contains("visited room 0")),
        "a previously visited room came back with {} rows",
        history.cached().len()
    );
}

#[test]
fn visited_room_markdown_stays_warm_after_five_rooms_and_a_revision() {
    let (mut state, rooms) = rooms_with_history(5);
    let initial = snapshot(state.clone(), 1);
    let mut history = History::default();
    history.enable_background().unwrap();
    let mut thumbnails = Thumbnails::default();
    await_layout(
        &mut history,
        Arc::clone(&initial),
        rooms[0],
        80,
        1_800_000_010_000,
        &mut thumbnails,
    );
    let original = history
        .cached()
        .iter()
        .find_map(|line| {
            line.raw_markdown
                .as_ref()
                .map(|(_, source)| Arc::clone(source))
        })
        .unwrap();
    for room in &rooms[1..] {
        await_layout(
            &mut history,
            Arc::clone(&initial),
            *room,
            80,
            1_800_000_010_000,
            &mut thumbnails,
        );
    }
    state
        .set_room_notes(rooms[0], "new global revision without a history change")
        .unwrap();
    let updated = snapshot(state, 2);
    await_layout(
        &mut history,
        updated,
        rooms[0],
        80,
        1_800_000_010_000,
        &mut thumbnails,
    );
    let reused = history
        .cached()
        .iter()
        .find_map(|line| line.raw_markdown.as_ref().map(|(_, source)| source))
        .unwrap();
    assert!(
        Arc::ptr_eq(&original, reused),
        "returning after a revision must not parse the first room's Markdown again"
    );
}

#[test]
fn deleting_a_visited_room_releases_its_cached_layout() {
    let (mut state, rooms) = rooms_with_history(2);
    let initial = snapshot(state.clone(), 1);
    let mut history = History::default();
    history.enable_background().unwrap();
    let mut thumbnails = Thumbnails::default();
    await_layout(
        &mut history,
        Arc::clone(&initial),
        rooms[0],
        80,
        1_800_000_010_000,
        &mut thumbnails,
    );
    let deleted_rows = Arc::downgrade(&history.lines);
    await_layout(
        &mut history,
        initial,
        rooms[1],
        80,
        1_800_000_010_000,
        &mut thumbnails,
    );
    state.delete_room(rooms[0], 1_800_000_010_000).unwrap();
    await_layout(
        &mut history,
        snapshot(state, 2),
        rooms[1],
        80,
        1_800_000_010_000,
        &mut thumbnails,
    );
    assert!(
        deleted_rows.upgrade().is_none(),
        "deleted room rows must be released by both UI and worker caches"
    );
    assert!(history
        .cached()
        .iter()
        .any(|line| line.text.contains("visited room 1")));
}
