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
    use crate::messaging::control::{server::DevCall, Request};
    let mut n = 0u64;
    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
        let (reply, _response) = mpsc::sync_channel(1);
        let call = DevCall {
            request: Request {
                id: format!("status-{n}"),
                method: "message.status".into(),
                params: serde_json::json!({"message": "1"}),
            },
            reply,
        };
        if commands.send((0, BusCommand::Dev(call))).is_err() {
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
    let dir = std::env::temp_dir().join(format!(
        "bus-send-latency-{}-{}",
        std::process::id(),
        crate::messaging::storage::io::now_ns()
    ));
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
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(ui.send_intent.is_some() || !ui.pending.is_empty());
    let prompts_before = seed.requests().count();

    // No further input: the one Enter must deliver the message on its own.
    let sent = tick_until(&mut ui, Duration::from_secs(5), |ui| {
        ui.send_intent.is_none()
            && ui.locals[&master].text.text.is_empty()
            && ui.snapshot.state.requests().count() > prompts_before
    });
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    poller.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let sent = sent.expect("Enter with no further input never sent the message");
    eprintln!("enter -> sent: {sent:?}");
    assert!(
        sent < Duration::from_millis(150),
        "Enter took {sent:?} to send; the draft-save round trips must not hold it"
    );
}
