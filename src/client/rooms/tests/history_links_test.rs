use super::*;
use crate::client::compositor::ClientShellAction;
use crossterm::event::{MouseButton::Left, MouseEventKind};
use ratatui::style::Modifier;

const LINK_FG: ratatui::style::Color = ratatui::style::Color::Rgb(90, 150, 255);

/// A short directory, so its paths fit on one history row.
fn scratch(label: &str) -> std::path::PathBuf {
    let root = if cfg!(unix) {
        std::path::PathBuf::from("/tmp")
    } else {
        std::env::temp_dir()
    };
    let dir = root.join(format!("bus-l-{label}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Shell actions from one pointer event.
fn actions(ui: &mut BusUi, kind: MouseEventKind, column: u16, row: u16) -> Vec<ClientShellAction> {
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
    outcome.actions
}

/// Press and release on one cell.
fn click(ui: &mut BusUi, (column, row): (u16, u16)) -> Vec<ClientShellAction> {
    let mut all = actions(ui, MouseEventKind::Down(Left), column, row);
    all.extend(actions(ui, MouseEventKind::Up(Left), column, row));
    all
}

fn opened(actions: &[ClientShellAction]) -> Option<String> {
    actions.iter().find_map(|action| match action {
        ClientShellAction::OpenPath(path) => Some(path.display().to_string()),
        ClientShellAction::OpenSafeWebUrl(url) => Some(url.clone()),
        _ => None,
    })
}

/// Lays out, lets the background path checks finish, and paints.
fn settled_screen(ui: &mut BusUi) -> ratatui::buffer::Buffer {
    ui.compute_view(120, 40);
    ui.link_paths.settle();
    ui.compute_view(120, 40);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 120, 40));
    ui.render(&mut buffer);
    buffer
}

fn is_link(buffer: &ratatui::buffer::Buffer, (column, row): (u16, u16)) -> bool {
    let cell = &buffer[(column, row)];
    cell.fg == LINK_FG && cell.modifier.contains(Modifier::UNDERLINED)
}

#[test]
fn attachments_read_as_links_without_brackets_and_open_on_click() {
    let dir = scratch("attachment");
    let doc = dir.join("lovart_产品设计文档.md");
    std::fs::write(&doc, "# doc").unwrap();
    let (mut ui, room, agent) = fixture();
    exchange_with_files(&mut ui, room, agent, std::slice::from_ref(&doc));
    let buffer = settled_screen(&mut ui);

    let lines = ui.history.cached();
    let file = lines
        .iter()
        .position(|line| line.text == "lovart_产品设计文档.md")
        .expect("attachment row without brackets");
    assert_eq!(lines[file - 1].text, "see attached", "follows its message");
    assert_eq!(lines[file + 1].text, "", "a blank row before the reply");
    assert!(
        lines[file + 2].text.contains("author"),
        "then the reply header"
    );

    let at = locate(&ui, "lovart");
    assert!(is_link(&buffer, at));
    assert!(is_link(&buffer, (at.0 + 3, at.1)));
    assert_eq!(opened(&click(&mut ui, at)), Some(doc.display().to_string()));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn only_an_existing_path_in_a_reply_is_underlined_and_a_click_opens_it() {
    let dir = scratch("reply");
    let doc = dir.join("lovart_产品设计文档.md");
    std::fs::write(&doc, "# doc").unwrap();
    let (mut ui, room, agent) = fixture();
    let reply = format!(
        "Sent `files: [\"{}\"]` and `{}/missing.md`.",
        doc.display(),
        dir.display()
    );
    saved_exchange(&mut ui, room, agent, "send it", &reply);
    let buffer = settled_screen(&mut ui);

    let path = doc.display().to_string();
    let at = locate(&ui, &path);
    assert!(is_link(&buffer, at), "the path is a link");
    assert!(
        !is_link(&buffer, (at.0 - 2, at.1)),
        "the quote and bracket are not"
    );
    let label = locate(&ui, "files:");
    assert!(!is_link(&buffer, label));
    assert_eq!(
        buffer[label].fg,
        ratatui::style::Color::Cyan,
        "still inline code"
    );
    let missing = locate(&ui, "missing.md");
    assert!(!is_link(&buffer, missing), "a path that does not exist");

    let actions = click(&mut ui, (at.0 + 4, at.1));
    assert_eq!(opened(&actions), Some(path.clone()));
    assert!(
        !actions
            .iter()
            .any(|action| matches!(action, ClientShellAction::ClipboardWrite(_))),
        "a click copies nothing"
    );
    // Clicking outside a link opens nothing.
    assert_eq!(opened(&click(&mut ui, missing)), None);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dragging_across_a_link_copies_plain_text_and_opens_nothing() {
    let dir = scratch("drag");
    let doc = dir.join("plan.md");
    std::fs::write(&doc, "x").unwrap();
    let (mut ui, room, agent) = fixture();
    saved_exchange(
        &mut ui,
        room,
        agent,
        "go",
        &format!("see {} now", doc.display()),
    );
    settled_screen(&mut ui);
    let (x, y) = locate(&ui, "see ");
    let end = locate(&ui, " now");
    let end = (end.0 - 1, end.1);
    let mut all = actions(&mut ui, MouseEventKind::Down(Left), x, y);
    all.extend(actions(&mut ui, MouseEventKind::Drag(Left), end.0, end.1));
    all.extend(actions(&mut ui, MouseEventKind::Up(Left), end.0, end.1));
    assert_eq!(opened(&all), None);
    let copied = all.iter().find_map(|action| match action {
        ClientShellAction::ClipboardWrite(bytes) => String::from_utf8(bytes.clone()).ok(),
        _ => None,
    });
    assert_eq!(copied, Some(format!("see {}", doc.display())));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn urls_and_relative_paths_from_the_agent_cwd_open_on_click() {
    let dir = scratch("relative");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/main.rs"), "fn main() {}").unwrap();
    let (mut ui, room, _) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let agent = snapshot
        .state
        .create_agent(room, "worker", Provider::Codex, dir.clone(), None)
        .unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    saved_exchange(
        &mut ui,
        room,
        agent,
        "where?",
        "In src/main.rs:12:5, docs at https://example.test/a.",
    );
    let buffer = settled_screen(&mut ui);
    let path = locate(&ui, "src/main.rs");
    assert!(is_link(&buffer, path));
    assert!(
        is_link(&buffer, (path.0 + 13, path.1)),
        ":12:5 is underlined too"
    );
    assert_eq!(
        opened(&click(&mut ui, path)),
        Some(dir.join("src/main.rs").display().to_string())
    );
    let url = locate(&ui, "https://");
    assert_eq!(
        opened(&click(&mut ui, url)),
        Some("https://example.test/a".into())
    );
    std::fs::remove_dir_all(dir).unwrap();
}
