use super::*;
use std::time::{Duration, Instant};

/// A real coordinator with one room whose draft already addresses an agent.
fn live_room() -> (BusUi, RoomId, std::path::PathBuf) {
    let mut seed = BusState::default();
    let master = seed.ensure_master_room();
    let agent = seed
        .create_agent(
            master,
            "lead",
            Provider::ClaudeCode,
            "/project".into(),
            None,
        )
        .unwrap();
    seed.set_draft_recipients(master, [agent]).unwrap();
    let dir = crate::utils::test_temp::unique_temp_path("bus-send-first-enter");
    let (handle, _commands) = BusHandle::start_for_test(dir.clone(), &seed);
    let mut ui = BusUi::new(handle.snapshot().unwrap());
    ui.handle = Some(handle);
    ui.open_room(master);
    tick_until(&mut ui, |ui| ui.pending.is_empty()).expect("navigation settles");
    (ui, master, dir)
}

fn tick_until(ui: &mut BusUi, done: impl Fn(&BusUi) -> bool) -> Option<Duration> {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        ui.tick();
        if done(ui) {
            return Some(start.elapsed());
        }
        std::thread::sleep(ui.tick_interval());
    }
    None
}

fn type_keys(ui: &mut BusUi, text: &str, tick_between: bool) {
    for c in text.chars() {
        key(ui, KeyCode::Char(c), KeyModifiers::NONE);
        if tick_between {
            ui.tick();
        }
    }
}

fn sent_texts(ui: &BusUi) -> Vec<String> {
    ui.snapshot
        .state
        .requests()
        .map(|request| request.prompt.text.clone())
        .collect()
}

fn settled(ui: &BusUi) -> bool {
    ui.send_intent.is_none() && ui.pending.is_empty()
}

fn assert_no_empty_rejection(ui: &BusUi) {
    let empty = "no text or files";
    assert!(
        !ui.error.as_deref().is_some_and(|e| e.contains(empty)),
        "error: {:?}",
        ui.error
    );
    assert!(
        !ui.toast.as_ref().is_some_and(|t| t.message.contains(empty)),
        "toast: {:?}",
        ui.toast.as_ref().map(|t| &t.message)
    );
}

#[test]
fn typed_text_sends_on_the_first_immediate_enter() {
    for tick_between in [false, true] {
        let (mut ui, room, dir) = live_room();
        type_keys(&mut ui, "I see", tick_between);
        key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
        let sent = tick_until(&mut ui, |ui| settled(ui) && !sent_texts(ui).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
        sent.expect("one Enter never sent the visible text");
        assert_eq!(sent_texts(&ui), ["I see"]);
        assert_eq!(ui.locals[&room].text.text, "");
        assert_no_empty_rejection(&ui);
    }
}

/// A developer unsure whether the last message went out presses Enter again on
/// the now-empty composer, then types the next message. That Enter has
/// nothing to send; it must not leave "no text or files" over the new text.
#[test]
fn enter_on_an_empty_composer_after_a_send_is_not_a_rejected_send() {
    let (mut ui, room, dir) = live_room();
    type_keys(&mut ui, "first", false);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    tick_until(&mut ui, |ui| {
        settled(ui) && ui.locals[&room].text.text.is_empty()
    })
    .expect("first message sends");
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    tick_until(&mut ui, settled).expect("extra Enter settles");
    type_keys(&mut ui, "I see", false);
    ui.tick();
    assert_no_empty_rejection(&ui);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    let sent = tick_until(&mut ui, |ui| settled(ui) && sent_texts(ui).len() == 2);
    let _ = std::fs::remove_dir_all(&dir);
    sent.expect("the next message never sent on its first Enter");
    let mut texts = sent_texts(&ui);
    texts.sort();
    assert_eq!(texts, ["I see", "first"]);
    assert_no_empty_rejection(&ui);
}
