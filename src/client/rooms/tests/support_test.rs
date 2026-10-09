use super::*;

pub(super) fn fixture() -> (BusUi, RoomId, AgentId) {
    let mut state = BusState::default();
    let room = state.create_room("room").unwrap();
    let agent = state
        .create_agent(
            room,
            "author",
            Provider::Codex,
            "/project".into(),
            Some("main".into()),
        )
        .unwrap();
    let ui = BusUi::new(Arc::new(BusSnapshot {
        state,
        revision: 0,
        last_command_id: 0,
        error: None,
    }));
    (ui, room, agent)
}

pub(super) fn key(ui: &mut BusUi, code: KeyCode, modifiers: KeyModifiers) {
    ui.input(
        &RawInputEvent::Key(TerminalKey::new(code, modifiers)),
        false,
        &mut Default::default(),
    );
}

/// The three ways terminals report a shifted symbol: the bare symbol, the
/// symbol with SHIFT, and the US base key with SHIFT plus the shifted codepoint.
pub(super) fn shifted_symbol_encodings(symbol: char, base: char) -> [TerminalKey; 3] {
    [
        TerminalKey::new(KeyCode::Char(symbol), KeyModifiers::NONE),
        TerminalKey::new(KeyCode::Char(symbol), KeyModifiers::SHIFT),
        TerminalKey::new(KeyCode::Char(base), KeyModifiers::SHIFT)
            .with_shifted_codepoint(symbol as u32),
    ]
}

pub(super) fn composer_rect(ui: &BusUi) -> ratatui::layout::Rect {
    ui.view
        .hits
        .iter()
        .find(|hit| hit.action == render::Action::Composer)
        .expect("composer hit area")
        .rect
}

pub(super) fn room_screen(ui: &mut BusUi, cols: u16, rows: u16) -> String {
    ui.compute_view(cols, rows);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, cols, rows));
    ui.render(&mut buffer);
    buffer.content.iter().map(|cell| cell.symbol()).collect()
}

pub(super) fn set_sidebar_agent_status(ui: &mut BusUi, agent: AgentId, status: RuntimeStatus) {
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.state.confirm_hook_setup(agent).unwrap();
    snapshot.state.observe_status(agent, status, 1).unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
}

pub(super) fn rendered_agent_status_colors(
    ui: &mut BusUi,
    agent: AgentId,
    expected: &str,
) -> Vec<ratatui::style::Color> {
    ui.compute_view(100, 30);
    let status = ui
        .view
        .hits
        .iter()
        .filter(|hit| hit.action == render::Action::Agent(agent))
        .max_by_key(|hit| hit.rect.x)
        .expect("agent status hit")
        .rect;
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
    ui.render(&mut buffer);
    assert_eq!(
        (0..status.width)
            .map(|offset| buffer[(status.x + offset, status.y)].symbol())
            .collect::<String>(),
        expected
    );
    (0..status.width)
        .map(|offset| buffer[(status.x + offset, status.y)].fg)
        .collect()
}

pub(super) fn shell_with(ui: BusUi) -> crate::client::compositor::ClientShellState {
    let mut shell = crate::client::compositor::ClientShellState::new(
        crate::client::compositor::ClientShellConfig::from_config(
            &crate::utils::config::Config::default(),
        ),
    );
    shell.bus = Some(ui);
    shell.snapshot = Some(Box::new(crate::client::compositor::tests::snapshot()));
    shell.pane_surface = Some(crate::client::compositor::tests::surface());
    shell.compose(100, 30);
    shell
}

pub(super) fn forwards_ctrl_c_to(
    outcome: &crate::client::compositor::ClientShellInput,
    pane: &str,
) -> bool {
    outcome.requests.iter().any(|r| {
        matches!(
            r,
            crate::protocol::wire::ClientMessage::ClientShellPaneInput { pane_id, events }
                if pane_id == pane
                    && matches!(
                        &events[..],
                        [crate::protocol::wire::ClientPaneInputEvent::Key {
                            code: crate::protocol::wire::ClientKeyCode::Char('c'),
                            modifiers,
                            ..
                        }] if *modifiers == KeyModifiers::CONTROL.bits()
                    )
        )
    })
}

pub(super) fn composer_rows(ui: &mut BusUi, cols: u16, rows: u16) -> Vec<String> {
    ui.compute_view(cols, rows);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, cols, rows));
    ui.render(&mut buffer);
    let rect = composer_rect(ui);
    (rect.y..rect.bottom())
        .map(|y| {
            (rect.x..rect.right())
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

pub(super) fn pointer(
    ui: &mut BusUi,
    kind: crossterm::event::MouseEventKind,
    column: u16,
    row: u16,
) -> Option<String> {
    let mut outcome = crate::client::compositor::ClientShellInput::default();
    ui.input(
        &RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }),
        false,
        &mut outcome,
    );
    outcome.actions.into_iter().find_map(|action| match action {
        crate::client::compositor::ClientShellAction::ClipboardWrite(bytes) => {
            String::from_utf8(bytes).ok()
        }
        _ => None,
    })
}

pub(super) fn drag_copy(ui: &mut BusUi, from: (u16, u16), to: (u16, u16)) -> Option<String> {
    use crossterm::event::{MouseButton::Left, MouseEventKind::*};
    assert_eq!(pointer(ui, Down(Left), from.0, from.1), None);
    assert_eq!(pointer(ui, Drag(Left), to.0, to.1), None);
    pointer(ui, Up(Left), to.0, to.1)
}

pub(super) fn mouse(ui: &mut BusUi, kind: crossterm::event::MouseEventKind, column: u16, row: u16) {
    ui.input(
        &RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }),
        false,
        &mut Default::default(),
    );
}

pub(super) fn screen_rows(ui: &mut BusUi, cols: u16, rows: u16) -> Vec<String> {
    ui.compute_view(cols, rows);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, cols, rows));
    ui.render(&mut buffer);
    (0..rows)
        .map(|y| (0..cols).map(|x| buffer[(x, y)].symbol()).collect())
        .collect()
}

pub(super) fn checklist(lines: usize) -> String {
    (1..=lines)
        .map(|line| format!("- [ ] item {line:02}"))
        .collect::<Vec<_>>()
        .join("\n")
}

// Persisted history and image fixtures shared by the history areas.
// Persisted request fixtures: all prompts and finals already exist in this
// format, even though the original UI only displayed the latest caches.
pub(super) fn saved_history(
    ui: &mut BusUi,
    room: RoomId,
    agent: AgentId,
    count: usize,
) -> Vec<RequestId> {
    let mut snapshot = (*ui.snapshot).clone();
    let mut requests = Vec::new();
    for i in 0..count {
        snapshot.state.set_draft_recipients(room, [agent]).unwrap();
        snapshot
            .state
            .set_draft_text(room, &format!("prompt-{i:02}"))
            .unwrap();
        requests.push(
            snapshot
                .state
                .submit_draft(room, 1000 + i as u64 * 1000)
                .unwrap()[0],
        );
    }
    let mut json = serde_json::to_value(&snapshot.state).unwrap();
    for (i, id) in requests.iter().enumerate() {
        let record = &mut json["requests"][id.0.to_string()];
        record["phase"] = "completed".into();
        record["completed_at_ms"] = (1500 + i as u64 * 1000).into();
        record["pending_final"] = serde_json::json!({
            "callback_id": format!("final-{i}"), "text": format!("answer-{i:02}"),
            "received_at_ms": 1500 + i as u64 * 1000,
            "provider_session_id": "session", "provider_turn_id": format!("turn-{i}")
        });
    }
    snapshot.state = serde_json::from_value(json).unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    requests
}

pub(super) fn is_reply(line: &history::Line) -> bool {
    matches!(
        line.raw_markdown,
        Some((history::MarkdownSource::Reply(_), _))
    )
}

pub(super) fn saved_exchange(
    ui: &mut BusUi,
    room: RoomId,
    agent: AgentId,
    prompt: &str,
    reply: &str,
) -> RequestId {
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.state.set_draft_recipients(room, [agent]).unwrap();
    snapshot.state.set_draft_text(room, prompt).unwrap();
    let request = snapshot.state.submit_draft(room, 1000).unwrap()[0];
    let mut json = serde_json::to_value(&snapshot.state).unwrap();
    let record = &mut json["requests"][request.0.to_string()];
    record["phase"] = "completed".into();
    record["completed_at_ms"] = 1500.into();
    record["pending_final"] = serde_json::json!({
        "callback_id": "final", "text": reply, "received_at_ms": 1500,
        "provider_session_id": "session", "provider_turn_id": "turn"
    });
    snapshot.state = serde_json::from_value(json).unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    request
}

/// The screen row of the first history line whose text contains `needle`, and
/// the column where `needle` starts.
pub(super) fn locate(ui: &BusUi, needle: &str) -> (u16, u16) {
    let text = ui.view.history_text;
    let (index, line) = ui
        .history
        .cached()
        .iter()
        .enumerate()
        .find(|(_, line)| line.text.contains(needle))
        .unwrap_or_else(|| panic!("no history row contains {needle:?}"));
    let column =
        unicode_width::UnicodeWidthStr::width(&line.text[..line.text.find(needle).unwrap()]);
    (
        text.x + column as u16,
        text.y + (index - ui.main_scroll) as u16,
    )
}

pub(super) fn complete_requests(
    ui: &mut BusUi,
    completions: impl IntoIterator<Item = (RequestId, &'static str, u64)>,
) {
    let mut snapshot = (*ui.snapshot).clone();
    let mut json = serde_json::to_value(&snapshot.state).unwrap();
    for (id, text, received_at_ms) in completions {
        let record = &mut json["requests"][id.0.to_string()];
        record["phase"] = "completed".into();
        record["completed_at_ms"] = received_at_ms.into();
        record["pending_final"] = serde_json::json!({
            "callback_id": format!("final-{}", id.0), "text": text,
            "received_at_ms": received_at_ms,
            "provider_session_id": "session", "provider_turn_id": format!("turn-{}", id.0)
        });
    }
    snapshot.state = serde_json::from_value(json).unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
}

pub(super) fn exchange_with_files(
    ui: &mut BusUi,
    room: RoomId,
    agent: AgentId,
    files: &[std::path::PathBuf],
) {
    saved_exchange(ui, room, agent, "see attached", "ok");
    let mut snapshot = (*ui.snapshot).clone();
    let mut json = serde_json::to_value(&snapshot.state).unwrap();
    for (_, record) in json["requests"].as_object_mut().unwrap() {
        record["prompt"]["files"] = serde_json::json!(files);
    }
    snapshot.state = serde_json::from_value(json).unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
}

pub(super) fn thumbnail_dir(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "bus-history-thumbnails-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

pub(super) fn png(dir: &std::path::Path, name: &str, size: (u32, u32)) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    image::RgbaImage::new(size.0, size.1).save(&path).unwrap();
    path
}

pub(super) const CELL: crate::protocol::kitty::HostCellSize =
    crate::protocol::kitty::HostCellSize {
        width_px: 10,
        height_px: 20,
    };
