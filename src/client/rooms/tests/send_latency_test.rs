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
    const WAIT: Duration = Duration::from_secs(60);
    let dir = crate::utils::test_temp::unique_temp_path("bus-send-latency");
    let (seed, master, _) = busy_session();
    let (handle, commands, saves) = BusHandle::start_for_test_observing_saves(dir.clone(), &seed);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let poller = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || poll_status(commands, stop))
    };
    let mut ui = BusUi::new(handle.snapshot().unwrap());
    ui.handle = Some(handle);
    ui.open_room(master);
    let navigation = tick_until(&mut ui, WAIT, |ui| ui.pending.is_empty());

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
    // A changed state.json timestamp cannot identify a blocking composer save:
    // the first archive tick can replace it after navigation was acknowledged.
    // Measure saves inside each serialized handler, separately from ticks.
    let enter = Instant::now();
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    let had_intent = ui.send_intent.is_some() || !ui.pending.is_empty();
    let prompts_before = seed.requests().count();

    // No further input: the one Enter must deliver the complete text on its own.
    let sent = tick_until(&mut ui, WAIT, |ui| {
        ui.send_intent.is_none()
            && ui.pending.is_empty()
            && ui.locals[&master].text.text.is_empty()
            && ui.snapshot.state.requests().count() > prompts_before
    });
    let elapsed = enter.elapsed();
    let sent_texts = ui
        .snapshot
        .state
        .requests()
        .filter(|request| request.prompt.text == "please review the draft")
        .count();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    poller.join().unwrap();

    // Positive control: the real startup archive must register a full save.
    // Its receipt may follow the command snapshot the UI already consumed.
    let mut observed = saves.try_iter().collect::<Vec<_>>();
    let deadline = Instant::now() + WAIT;
    while sent.is_some()
        && !observed
            .iter()
            .any(|save| save.operation == "tick" && save.full_saves > 0)
    {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match saves.recv_timeout(remaining) {
            Ok(save) => observed.push(save),
            Err(_) => break,
        }
    }
    observed.extend(saves.try_iter());

    // Fold the journal and stop the owned coordinator before removing its files.
    let handle = ui.handle.as_ref().unwrap();
    let shutdown_id = u64::MAX;
    let shutdown = handle.try_send(shutdown_id, BusCommand::Shutdown).is_ok();
    let deadline = Instant::now() + WAIT;
    let mut stopped = false;
    while shutdown && Instant::now() < deadline {
        if handle
            .snapshot()
            .is_some_and(|snapshot| snapshot.last_command_id == shutdown_id)
        {
            stopped = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    drop(ui);
    if stopped {
        let _ = std::fs::remove_dir_all(&dir);
    }

    assert!(navigation.is_some(), "navigation never settled");
    assert!(had_intent, "Enter did not queue its send intent");
    sent.expect("Enter with no further input never sent the message");
    assert!(
        stopped,
        "owned coordinator did not stop; evidence at {dir:?}"
    );
    assert_eq!(sent_texts, 1, "one Enter must send the exact draft once");
    eprintln!("enter -> sent: {elapsed:?}; save receipts: {observed:?}");
    assert!(
        observed
            .iter()
            .any(|save| save.operation == "tick" && save.full_saves > 0),
        "the startup archive did not exercise the save observer: {observed:?}"
    );
    let composer = observed
        .iter()
        .filter(|save| {
            matches!(
                save.operation,
                "set_draft_text" | "submit" | "submit_queued"
            )
        })
        .collect::<Vec<_>>();
    assert!(composer
        .iter()
        .any(|save| save.operation == "set_draft_text"));
    assert_eq!(
        composer
            .iter()
            .filter(|save| save.operation.starts_with("submit"))
            .count(),
        1
    );
    for save in composer {
        assert!(
            save.command_id.is_some(),
            "composer receipt needs its command ID"
        );
        assert_eq!(
            save.full_saves, 0,
            "composer handler performed a full save: {save:?}"
        );
    }
}
