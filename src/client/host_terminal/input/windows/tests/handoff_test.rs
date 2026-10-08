use super::*;

#[cfg(windows)]
#[test]
fn windows_input_handoff_keeps_input_while_the_client_queue_is_full() {
    use crate::protocol::{
        ClientInputEvent, ClientKeyCode, ClientKeyKind, ClientKeySource, ClientMouseButton,
        ClientMouseKind,
    };

    let mouse = |kind, column| ClientInputEvent::Mouse {
        kind,
        column,
        row: 4,
        modifiers: 0,
    };
    let down = mouse(ClientMouseKind::Down(ClientMouseButton::Left), 1);
    let last_move = mouse(ClientMouseKind::Moved, 12);
    let last_drag = mouse(ClientMouseKind::Drag(ClientMouseButton::Left), 9);
    let scroll = mouse(ClientMouseKind::ScrollDown, 9);
    let up = mouse(ClientMouseKind::Up(ClientMouseButton::Left), 9);
    let shortcut = |kind| ClientInputEvent::Key {
        code: ClientKeyCode::Char('v'),
        modifiers: crossterm::event::KeyModifiers::CONTROL.bits(),
        kind,
        repeat_count: 1,
        generated_text: None,
        source: ClientKeySource::Synthesized,
    };
    let shortcut_press = shortcut(ClientKeyKind::Press);
    let shortcut_release = shortcut(ClientKeyKind::Release);
    let text = ClientInputEvent::TextCommit("5;37;15M".into());

    let (event_tx, mut event_rx) = mpsc::channel(1);
    event_tx.try_send(ClientLoopEvent::Timer).unwrap();
    let mut handoff = WindowsInputHandoff::default();
    for event in [
        mouse(ClientMouseKind::Moved, 10),
        last_move.clone(),
        down.clone(),
        mouse(ClientMouseKind::Drag(ClientMouseButton::Left), 2),
        mouse(ClientMouseKind::Drag(ClientMouseButton::Left), 5),
        last_drag.clone(),
        scroll.clone(),
        up.clone(),
        shortcut_press.clone(),
        shortcut_release.clone(),
        text.clone(),
    ] {
        handoff.push(vec![event]);
    }

    assert!(handoff.try_flush(&event_tx));
    let expected = vec![
        last_move,
        down,
        last_drag,
        scroll,
        up,
        shortcut_press,
        shortcut_release,
        text,
    ];
    assert_eq!(
        handoff.pending,
        expected
            .iter()
            .cloned()
            .map(|event| vec![event])
            .collect::<VecDeque<_>>()
    );
    assert!(matches!(event_rx.try_recv(), Ok(ClientLoopEvent::Timer)));

    let mut delivered = Vec::new();
    while !handoff.pending.is_empty() {
        assert!(handoff.try_flush(&event_tx));
        let Ok(ClientLoopEvent::StdinEvents(events)) = event_rx.try_recv() else {
            panic!("expected retained Windows input events");
        };
        assert_eq!(events.len(), 1, "logical input batches must stay separate");
        delivered.extend(events);
    }
    assert_eq!(delivered, expected);
}
