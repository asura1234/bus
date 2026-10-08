use super::*;
use crossterm::event::MouseEventKind::{ScrollDown, ScrollUp};

#[test]
fn late_reply_expansion_above_viewport_preserves_the_visible_anchor() {
    let (mut ui, room, author) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let mut requests = Vec::new();
    for index in 0..8 {
        snapshot.state.set_draft_recipients(room, [author]).unwrap();
        snapshot
            .state
            .set_draft_text(room, &format!("anchor-prompt-{index}"))
            .unwrap();
        requests.push(snapshot.state.submit_draft(room, 1_000 + index).unwrap()[0]);
    }
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.compute_view(100, 24);
    let anchor = "anchor-prompt-5";
    ui.main_scroll = ui
        .history
        .cached()
        .iter()
        .position(|line| line.text == anchor)
        .unwrap();
    ui.history_follow_tail = false;

    complete_requests(
        &mut ui,
        [(requests[0], "one\ntwo\nthree\nfour\nfive\nsix", 9_000)],
    );
    ui.compute_view(100, 24);

    assert_eq!(ui.history.cached()[ui.main_scroll].text, anchor);
}

#[test]
fn saved_round_trips_scroll_oldest_to_newest_with_fixed_chrome() {
    let (mut ui, room, agent) = fixture();
    saved_history(&mut ui, room, agent, 20);
    let bottom = room_screen(&mut ui, 100, 30);
    assert!(
        bottom.contains("answer-19"),
        "newest reply is visible on opening"
    );
    assert!(!bottom.contains("prompt-00"));
    let composer = composer_rect(&ui);
    let mut seen = String::new();
    for _ in 0..=ui.view.history_max_scroll {
        seen.push_str(&room_screen(&mut ui, 100, 30));
        mouse(&mut ui, ScrollUp, 40, 12);
        ui.compute_view(100, 30);
        assert_eq!(composer_rect(&ui), composer);
    }
    let top = room_screen(&mut ui, 100, 30);
    assert!(top.find("prompt-00").unwrap() < top.find("answer-00").unwrap());
    for i in 0..20 {
        assert!(seen.contains(&format!("prompt-{i:02}")));
        assert!(seen.contains(&format!("answer-{i:02}")));
    }
    // New arrivals do not yank the viewport away while reading older messages.
    saved_history(&mut ui, room, agent, 1);
    let after = room_screen(&mut ui, 100, 30);
    assert!(after.contains("prompt-00"));
    assert_eq!(ui.main_scroll, 0);
    for _ in 0..=ui.view.history_max_scroll {
        mouse(&mut ui, ScrollDown, 40, 12);
        ui.compute_view(100, 30);
    }
    assert_eq!(ui.main_scroll, ui.view.history_max_scroll);
    ui.open_room(room);
    assert!(room_screen(&mut ui, 100, 30).contains("answer-19"));
}
