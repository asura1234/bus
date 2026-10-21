use super::*;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// A long-running session: about 1,300 sent messages, as in the 5 MB
/// `state.json` where Enter waited behind every draft save.
fn busy_session() -> (BusState, RoomId, AgentId) {
    let mut state = BusState::default();
    let master = state.ensure_master_room();
    let room = state.create_room("work").unwrap();
    let agent = state
        .create_agent(
            master,
            "lead",
            Provider::ClaudeCode,
            "/project".into(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(agent, room).unwrap();
    state.set_draft_recipients(master, [agent]).unwrap();
    let body = "a sentence a human typed to an orchestrator. ".repeat(70);
    for i in 0..1_300u64 {
        state
            .set_draft_text(master, &format!("{i} {body}"))
            .unwrap();
        state.submit_draft(master, i).unwrap();
    }
    (state, master, agent)
}

/// Orchestrators wait on `bus send --async` with `message.status` reads,
/// together about 20 a second.
fn poll_status(
    commands: mpsc::SyncSender<(u64, BusCommand)>,
    stop: Arc<std::sync::atomic::AtomicBool>,
) {
    use crate::messaging::control::{server::ControlCall, Request};
    let mut n = 0u64;
    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
        let (reply, _response) = mpsc::sync_channel(1);
        let call = ControlCall {
            request: Request {
                id: format!("status-{n}"),
                method: "message.status".into(),
                params: serde_json::json!({"message": "1"}),
            },
            reply,
            deadline: None,
            _poll_permit: None,
        };
        if commands.send((0, BusCommand::Control(call))).is_err() {
            return;
        }
        n += 1;
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The client loop ticks Bus on its timer whether or not keys arrive.
fn tick_until(
    ui: &mut BusUi,
    deadline: Duration,
    done: impl Fn(&BusUi) -> bool,
) -> Option<Duration> {
    let start = Instant::now();
    while start.elapsed() < deadline {
        ui.tick();
        if done(ui) {
            return Some(start.elapsed());
        }
        std::thread::sleep(ui.tick_interval());
    }
    None
}

#[test]
fn one_enter_sends_promptly_in_a_busy_master_room_without_further_input() {
    let dir = crate::utils::test_temp::unique_temp_path("bus-send-latency");
    let (seed, master, _) = busy_session();
    let (handle, commands) = BusHandle::start_for_test(dir.clone(), &seed);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let poller = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || poll_status(commands, stop))
    };
    let mut ui = BusUi::new(handle.snapshot().unwrap());
    ui.handle = Some(handle);
    ui.open_room(master);
    // Settle the navigation commands before typing, like a human reading first.
    tick_until(&mut ui, Duration::from_secs(10), |ui| ui.pending.is_empty());

    // Type at a brisk human pace with the 100 ms client tick interleaved.
    let started = Instant::now();
    for c in "please review the draft".chars() {
        ui.input(
            &RawInputEvent::Text(crate::protocol::keys::TextCommit::new(c)),
            false,
            &mut Default::default(),
        );
        if started.elapsed().as_millis() % 100 < 60 {
            ui.tick();
        }
        std::thread::sleep(Duration::from_millis(60));
    }
    // The bug this guards: Enter waited behind full `state.json` saves (170-270 ms
    // each for this session in a debug build). Assert that no full save sits
    // between Enter and the send rather than timing it, since wall time here also
    // counts the journal fsync and scheduling, which a busy machine stretches.
    let state_file = dir.join("state.json");
    let saved_before = state_file_version(&state_file);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(ui.send_intent.is_some() || !ui.pending.is_empty());
    let prompts_before = seed.requests().count();

    // No further input: the one Enter must deliver the message on its own.
    let sent = tick_until(&mut ui, Duration::from_secs(5), |ui| {
        ui.send_intent.is_none()
            && ui.locals[&master].text.text.is_empty()
            && ui.snapshot.state.requests().count() > prompts_before
    });
    let saved_after = state_file_version(&state_file);
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    poller.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let sent = sent.expect("Enter with no further input never sent the message");
    eprintln!("enter -> sent: {sent:?}");
    assert_eq!(
        saved_after, saved_before,
        "Enter waited on a full state.json save before the send landed ({sent:?})"
    );
}

/// When and how large `state.json` was last written; a full save replaces it.
fn state_file_version(path: &std::path::Path) -> (std::time::SystemTime, u64) {
    let metadata = std::fs::metadata(path).unwrap();
    (metadata.modified().unwrap(), metadata.len())
}
