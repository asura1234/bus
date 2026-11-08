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
        let body = format!(
            "**Prompt {index}**\nRead src/client/rooms/history/exchange.rs and https://example.com/long/path/to/documentation\nKeep output identical.\n\n## Result\n\n- **First** implementation detail\n- Second detail with `code`\n- Third detail\n\nSee https://example.com/long/path/to/documentation and src/client/rooms/history/exchange.rs.\n\n```rust\nlet answer = 42;\n```\n{}",
            (0..4)
                .map(|line| format!("detail line {line} keeps the history large"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let id = state
            .submit_message_from(
                room,
                Draft {
                    text: body,
                    files: vec![],
                    recipient_ids: [agent].into(),
                },
                Author::Developer,
                1_800_000_000_000 + index,
            )
            .unwrap()[0];
        state.finish_for_test(id, "settled", 1_800_000_003_000 + index);
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
    assert_eq!(
        ui.history
            .background
            .as_ref()
            .unwrap()
            .test_progress()
            .cancelled,
        0
    );
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
    let began = Instant::now();
    let mut last_progress = (0, 0, 0, 0);
    let mut progressed_at = began;
    let mut generation = None;
    loop {
        history.request_lines(Arc::clone(&snapshot), room, width, now, thumbnails);
        if history.background.as_ref().unwrap().is_current(
            snapshot.revision,
            room,
            width,
            now,
            thumbnails.layout_key(),
        ) {
            eprintln!(
                "history layout elapsed_ms={} progress={:?}",
                began.elapsed().as_millis(),
                history.background.as_ref().unwrap().test_progress()
            );
            return;
        }
        let current = history
            .background
            .as_ref()
            .unwrap()
            .test_progress()
            .generation;
        if let Some(expected) = generation {
            assert_eq!(
                current, expected,
                "unchanged layout request restarted the worker"
            );
        } else {
            generation = Some(current);
        }
        assert_worker_progress(history, &mut last_progress, &mut progressed_at);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

fn assert_worker_progress(
    history: &History,
    previous: &mut (u64, u64, u64, u64),
    progressed_at: &mut Instant,
) {
    let progress = history.background.as_ref().unwrap().test_progress();
    let current = (
        progress.exchanges,
        progress.started,
        progress.finished,
        progress.cancelled,
    );
    let advanced = current.0 > previous.0 || current.1 > previous.1 || current.2 > previous.2;
    if advanced && current.3 == previous.3 {
        *progressed_at = Instant::now();
    }
    *previous = current;
    assert!(
        !progress.worker_stopped,
        "history worker stopped: {progress:?}"
    );
    // This is a liveness guard, not a host-speed benchmark. The same 2500
    // exchanges take 2s unloaded and over 30s with competing CPU work. A slow
    // but advancing renderer must not be mistaken for cancellation starvation.
    assert!(
        progressed_at.elapsed() < std::time::Duration::from_secs(30),
        "history worker made no progress for 30s: {progress:?}"
    );
}

#[test]
#[should_panic(expected = "history worker made no progress for 30s")]
fn cancelled_history_work_does_not_restart_the_liveness_clock() {
    let mut history = History::default();
    history.enable_background().unwrap();
    // Run the actual stale-generation cancellation check, without depending
    // on scheduling a resize between two exchanges of a live renderer.
    history
        .background
        .as_ref()
        .unwrap()
        .test_check_stale_generation();
    let progress = history.background.as_ref().unwrap().test_progress();
    assert_eq!(progress.exchanges, 1);
    assert_eq!(progress.cancelled, 1);
    let mut previous = (0, 0, 0, 0);
    let mut progressed_at = Instant::now() - std::time::Duration::from_secs(31);
    assert_worker_progress(&history, &mut previous, &mut progressed_at);
}

#[test]
#[should_panic(expected = "history worker made no progress for 30s")]
fn a_stalled_history_worker_still_fails_the_liveness_guard() {
    let mut history = History::default();
    history.enable_background().unwrap();
    let mut previous = (0, 0, 0, 0);
    let mut progressed_at = Instant::now() - std::time::Duration::from_secs(31);
    assert_worker_progress(&history, &mut previous, &mut progressed_at);
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
            text: "**Hello** 中文 👩‍💻\nA long https://example.com/a/b/c/d/e/f/g/h/i/j/k link\n[waiting-on-you]\n\n# Result\n\n> quoted **text**\n\n- one\n- two\n\n| a | b |\n|---|---|\n| alpha | beta |\n\n```rust\nlet n = 1;\n```\n\n[docs](https://example.com)".into(),
            files: vec!["/missing/notes.md".into()], recipient_ids: [agent].into(),
        }, Author::Developer, 1_800_000_000_000).unwrap()[0];
        state.finish_for_test(id, "settled", 1_800_000_001_000);
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
    let began = Instant::now();
    let mut last_progress = (0, 0, 0, 0);
    let mut progressed_at = began;
    let mut generation = None;
    let mut revision = 0;
    loop {
        revision += 1;
        history.request_lines(
            snapshot(state.clone(), revision),
            room,
            100,
            1_800_000_010_000,
            &mut thumbnails,
        );
        assert_eq!(history.rebuilt, 0);
        let progress = history.background.as_ref().unwrap().test_progress();
        if let Some(first) = generation {
            assert_eq!(
                progress.generation, first,
                "revision cancelled a cold layout"
            );
        } else {
            generation = Some(progress.generation);
        }
        assert_eq!(
            progress.cancelled, 0,
            "a cold layout was cancelled: {progress:?}"
        );
        // Exercise a revision change even if the first layout finished before
        // the test thread was scheduled again.
        if revision >= 2 && !history.cached().is_empty() {
            break;
        }
        assert_worker_progress(&history, &mut last_progress, &mut progressed_at);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(history.cached().len() >= 35_000);
    eprintln!(
        "cold revisions={revision} elapsed_ms={} progress={:?}",
        began.elapsed().as_millis(),
        history.background.as_ref().unwrap().test_progress()
    );
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
    let dir = crate::utils::test_temp::unique_temp_path("bus-history-image");
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
    let draft = || {
        Draft {
        text: "https://example.com/a/b/c/d/e/f/g/h/i/j/k/l/m\nRead src/client/rooms/history/exchange.rs"
            .into(),
        files: vec![],
        recipient_ids: [agent].into(),
    }
    };
    let request = state
        .submit_message_from(room, draft(), Author::Developer, 1_800_000_000_000)
        .unwrap()[0];
    state.finish_for_test(request, "settled", 1_800_000_001_000);
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
    assert_eq!(history.projected, 1);
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
        .rename_room(rooms[0], "renamed without a history change")
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

#[test]
fn filter_first_render_is_bounded_by_recent_records_in_a_50k_room() {
    let mut state = BusState::default();
    let room = state.create_room("50k").unwrap();
    let agent = state
        .create_agent(room, "worker", Provider::Codex, "/project".into(), None)
        .unwrap();
    for index in 0..50_000 {
        state
            .post_to_developer(
                room,
                agent,
                format!("recent-record-{index}"),
                vec![],
                1_800_000_000_000 + index,
            )
            .unwrap();
    }
    let mut history = History::default();
    let started = Instant::now();
    history.lines_filtered(
        &state,
        state.room(room).unwrap(),
        80,
        1,
        1_800_000_100_000,
        &mut Thumbnails::default(),
        crate::messaging::model::RoomHistoryFilters {
            to_you: true,
            from_you: false,
        },
    );
    assert!(history
        .cached()
        .iter()
        .any(|line| line.text == "recent-record-49999"));
    assert!(
        history.rebuilt <= 256,
        "first filtered frame rebuilt {} exchanges before showing the newest match",
        history.rebuilt
    );
    assert!(
        history.rendered <= 256,
        "first filtered frame parsed {} bodies",
        history.rendered
    );
    eprintln!(
        "50k first filtered frame: exchanges={} parsed={} elapsed_us={}",
        history.rebuilt,
        history.rendered,
        started.elapsed().as_micros()
    );
    assert!(!history
        .cached()
        .iter()
        .any(|line| line.text == "recent-record-0"));
    state
        .post_to_developer(
            room,
            agent,
            "new direct arrival".into(),
            vec![],
            1_800_000_100_001,
        )
        .unwrap();
    history.lines_filtered(
        &state,
        state.room(room).unwrap(),
        80,
        2,
        1_800_000_100_002,
        &mut Thumbnails::default(),
        crate::messaging::model::RoomHistoryFilters {
            to_you: true,
            from_you: false,
        },
    );
    assert!(history
        .cached()
        .iter()
        .any(|line| line.text == "new direct arrival"));
    assert_eq!(
        history.rendered, 1,
        "an arrival must reuse the older matching Markdown bodies"
    );
}

#[test]
fn filtered_window_can_extend_after_returning_to_cached_room() {
    let mut state = BusState::default();
    let room = state.create_room("paged").unwrap();
    let other = state.create_room("other").unwrap();
    let agent = state
        .create_agent(room, "worker", Provider::Codex, "/project".into(), None)
        .unwrap();
    for index in 0..600 {
        state
            .post_to_developer(
                room,
                agent,
                format!("page-record-{index}"),
                vec![],
                1000 + index,
            )
            .unwrap();
    }
    state
        .set_room_history_filters(
            room,
            crate::messaging::model::RoomHistoryFilters {
                to_you: true,
                from_you: false,
            },
        )
        .unwrap();
    let snapshot = snapshot(state, 1);
    let mut history = History::default();
    history.enable_background().unwrap();
    let mut thumbnails = Thumbnails::default();
    await_layout(
        &mut history,
        Arc::clone(&snapshot),
        room,
        80,
        2000,
        &mut thumbnails,
    );
    assert!(history
        .cached()
        .iter()
        .any(|line| line.text == "page-record-599"));
    assert!(!history
        .cached()
        .iter()
        .any(|line| line.text == "page-record-0"));
    await_layout(
        &mut history,
        Arc::clone(&snapshot),
        other,
        80,
        2000,
        &mut thumbnails,
    );
    await_layout(
        &mut history,
        Arc::clone(&snapshot),
        room,
        80,
        2000,
        &mut thumbnails,
    );
    assert!(
        history.load_older(),
        "returning to a cached filtered room must retain its older-history cursor"
    );
    await_layout(
        &mut history,
        Arc::clone(&snapshot),
        room,
        80,
        2000,
        &mut thumbnails,
    );
    assert!(history
        .cached()
        .iter()
        .any(|line| line.text == "page-record-100"));
    assert!(history.load_older());
    await_layout(&mut history, snapshot, room, 80, 2000, &mut thumbnails);
    assert!(history
        .cached()
        .iter()
        .any(|line| line.text == "page-record-0"));
    assert!(
        !history.load_older(),
        "fully loaded history has no older page"
    );
}

#[test]
fn sparse_filtered_window_reaches_matches_behind_recent_peer_traffic() {
    let mut state = BusState::default();
    let room = state.create_room("sparse").unwrap();
    let agent = state
        .create_agent(room, "worker", Provider::Codex, "/project".into(), None)
        .unwrap();
    state
        .post_to_developer(room, agent, "older direct match".into(), vec![], 1000)
        .unwrap();
    for index in 0..600 {
        state
            .submit_message_from(
                room,
                Draft {
                    text: format!("hidden peer {index}"),
                    files: vec![],
                    recipient_ids: [agent].into(),
                },
                Author::Agent(agent),
                1001 + index,
            )
            .unwrap();
    }
    state
        .set_room_history_filters(
            room,
            crate::messaging::model::RoomHistoryFilters {
                to_you: true,
                from_you: false,
            },
        )
        .unwrap();
    let snapshot = snapshot(state, 1);
    let mut history = History::default();
    history.enable_background().unwrap();
    let mut thumbnails = Thumbnails::default();
    await_layout(&mut history, snapshot, room, 80, 2000, &mut thumbnails);
    assert!(history
        .cached()
        .iter()
        .any(|line| line.text == "older direct match"));
    assert!(!history
        .cached()
        .iter()
        .any(|line| line.text.contains("hidden peer")));
}

#[test]
fn pagination_preserves_loaded_window_after_room_return() {
    let mut state = BusState::default();
    let room = state.create_room("paged").unwrap();
    let other = state.create_room("other").unwrap();
    let agent = state
        .create_agent(room, "worker", Provider::Codex, "/project".into(), None)
        .unwrap();
    for index in 0..600 {
        state
            .post_to_developer(
                room,
                agent,
                format!("page-record-{index}"),
                vec![],
                1000 + index,
            )
            .unwrap();
    }
    state
        .set_room_history_filters(
            room,
            crate::messaging::model::RoomHistoryFilters {
                to_you: true,
                from_you: false,
            },
        )
        .unwrap();
    let snapshot = snapshot(state, 1);
    let mut history = History::default();
    history.enable_background().unwrap();
    let mut thumbnails = Thumbnails::default();
    await_layout(
        &mut history,
        Arc::clone(&snapshot),
        room,
        80,
        2000,
        &mut thumbnails,
    );
    assert!(history.load_older());
    await_layout(
        &mut history,
        Arc::clone(&snapshot),
        room,
        80,
        2000,
        &mut thumbnails,
    );
    assert!(history
        .cached()
        .iter()
        .any(|line| line.text == "page-record-100"));

    await_layout(
        &mut history,
        Arc::clone(&snapshot),
        other,
        80,
        2000,
        &mut thumbnails,
    );
    await_layout(
        &mut history,
        Arc::clone(&snapshot),
        room,
        80,
        2000,
        &mut thumbnails,
    );

    assert!(
        history
            .cached()
            .iter()
            .any(|line| line.text == "page-record-100"),
        "returning to the room shrank the loaded window"
    );
}

#[test]
fn pagination_ignores_foreign_notices() {
    let mut state = BusState::default();
    let quiet = state.create_room("quiet").unwrap();
    let busy = state.create_room("busy").unwrap();
    let agent = state
        .create_agent(busy, "worker", Provider::Codex, "/project".into(), None)
        .unwrap();
    for index in 0..600 {
        state
            .post_to_developer(
                busy,
                agent,
                format!("foreign-record-{index}"),
                vec![],
                1000 + index,
            )
            .unwrap();
    }
    state
        .set_room_history_filters(
            quiet,
            crate::messaging::model::RoomHistoryFilters {
                to_you: true,
                from_you: false,
            },
        )
        .unwrap();
    let snapshot = snapshot(state, 1);
    let mut history = History::default();
    history.enable_background().unwrap();
    let mut thumbnails = Thumbnails::default();
    await_layout(&mut history, snapshot, quiet, 80, 2000, &mut thumbnails);
    assert!(history.cached().is_empty());
    assert_eq!(
        history.record_limit,
        super::super::window::PAGE_RECORDS,
        "foreign traffic triggered another page in a room with no older records"
    );
}

fn pagination_foreign_requests(archived: bool) {
    let mut state = BusState::default();
    let quiet = state.create_room("quiet").unwrap();
    let busy = state.create_room("busy").unwrap();
    let agent = state
        .create_agent(busy, "worker", Provider::Codex, "/project".into(), None)
        .unwrap();
    for index in 0..600 {
        let id = state
            .submit_message_from(
                busy,
                Draft {
                    text: format!("foreign-request-{index}"),
                    files: vec![],
                    recipient_ids: [agent].into(),
                },
                Author::Developer,
                1000 + index,
            )
            .unwrap()[0];
        state.finish_for_test(id, "foreign answer", 2000);
    }
    if archived {
        state.archive(1_000_000);
    }
    assert_eq!(state.requests().count(), 600);
    assert_eq!(state.history().len(), if archived { 600 } else { 0 });
    state
        .set_room_history_filters(
            quiet,
            crate::messaging::model::RoomHistoryFilters {
                to_you: true,
                from_you: false,
            },
        )
        .unwrap();
    let snapshot = snapshot(state, 1);
    let mut history = History::default();
    history.enable_background().unwrap();
    await_layout(
        &mut history,
        snapshot,
        quiet,
        80,
        2000,
        &mut Thumbnails::default(),
    );
    assert!(history.cached().is_empty());
    assert_eq!(
        history.record_limit,
        super::super::window::PAGE_RECORDS,
        "foreign requests triggered extra pages in an empty room (archived={archived})"
    );
}

#[test]
fn pagination_ignores_foreign_live_requests() {
    pagination_foreign_requests(false);
}

#[test]
fn pagination_ignores_foreign_archived_requests() {
    pagination_foreign_requests(true);
}

#[test]
fn filtered_large_request_archive_projects_only_recent_bodies() {
    let mut state = BusState::default();
    let room = state.create_room("large").unwrap();
    let other = state.create_room("other").unwrap();
    let agent = state
        .create_agent(room, "worker", Provider::Codex, "/project".into(), None)
        .unwrap();
    for index in 0..50_000 {
        let id = state
            .submit_message_from(
                room,
                Draft {
                    text: format!("archived prompt {index}"),
                    files: vec![],
                    recipient_ids: [agent].into(),
                },
                Author::Developer,
                1000 + index,
            )
            .unwrap()[0];
        state.finish_for_test(id, &format!("archived answer {index}"), 100_000);
    }
    state.archive(1_000_000);
    assert_eq!(state.history().len(), 50_000);
    let mut history = History::default();
    let filters = crate::messaging::model::RoomHistoryFilters {
        to_you: true,
        from_you: false,
    };
    history.lines_filtered(
        &state,
        state.room(room).unwrap(),
        80,
        1,
        1_000_000,
        &mut Thumbnails::default(),
        filters,
    );
    assert!(history
        .cached()
        .iter()
        .any(|line| line.text.contains("archived answer 49999")));
    assert_eq!(history.rebuilt, 256);
    assert_eq!(history.rendered, 256);
    assert!(!history
        .cached()
        .iter()
        .any(|line| line.text.contains("archived answer 0")));
    history.lines_filtered(
        &state,
        state.room(other).unwrap(),
        80,
        1,
        1_000_000,
        &mut Thumbnails::default(),
        filters,
    );
    assert!(history.cached().is_empty());
    assert!(!history.has_older());
}

#[test]
fn filtered_submission_status_rejects_stale_job_without_a_revision_change() {
    use super::super::LocalSendStatus;
    let mut state = BusState::default();
    let room = state.create_room("paged").unwrap();
    let agent = state
        .create_agent(room, "worker", Provider::Codex, "/project".into(), None)
        .unwrap();
    let mut newest = None;
    for index in 0..600 {
        let id = state
            .submit_message_from(
                room,
                Draft {
                    text: format!("outgoing {index}"),
                    files: vec![],
                    recipient_ids: [agent].into(),
                },
                Author::Developer,
                1000 + index,
            )
            .unwrap()[0];
        newest = Some(state.request(id).unwrap().prompt.id);
    }
    let filters = crate::messaging::model::RoomHistoryFilters {
        to_you: false,
        from_you: true,
    };
    let snapshot = snapshot(state, 7);
    let prompt = newest.unwrap();
    let pending = BTreeMap::from([(prompt, LocalSendStatus::Pending)]);
    let failed = BTreeMap::from([(prompt, LocalSendStatus::Failed)]);
    let mut history = History::default();
    history.enable_background().unwrap();
    let mut thumbnails = Thumbnails::default();
    history.request_lines_with_submissions(
        Arc::clone(&snapshot),
        room,
        80,
        2000,
        &mut thumbnails,
        filters,
        pending.clone(),
    );
    let started = Instant::now();
    while !history.poll() {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "pending job did not complete"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    // The completed Pending result is still in the mailbox. It must never be
    // applied after the same snapshot's local submission status becomes Failed.
    history.request_lines_with_submissions(
        Arc::clone(&snapshot),
        room,
        80,
        2000,
        &mut thumbnails,
        filters,
        failed.clone(),
    );
    assert!(!history
        .cached()
        .iter()
        .any(|line| line.text.contains("[pending]")));
    let started = Instant::now();
    loop {
        history.request_lines_with_submissions(
            Arc::clone(&snapshot),
            room,
            80,
            2000,
            &mut thumbnails,
            filters,
            failed.clone(),
        );
        if history.background.as_ref().unwrap().is_current(
            7,
            room,
            80,
            2000,
            thumbnails.layout_key(),
        ) {
            break;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "failed badge layout did not finish"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(
        history
            .cached()
            .iter()
            .filter(|line| line.text.contains("[failed]"))
            .count(),
        1
    );
    assert!(history
        .cached()
        .iter()
        .any(|line| line.text == "outgoing 599"));
    assert!(history.rebuilt <= 256);
    assert!(history.load_older());
    history.request_lines_with_submissions(
        Arc::clone(&snapshot),
        room,
        80,
        2000,
        &mut thumbnails,
        filters,
        failed,
    );
    // A filter change rejects that completed/running outgoing page as well.
    history.request_lines_with_submissions(
        snapshot,
        room,
        80,
        2000,
        &mut thumbnails,
        crate::messaging::model::RoomHistoryFilters {
            to_you: true,
            from_you: false,
        },
        pending,
    );
    assert!(history.cached().is_empty());
}
