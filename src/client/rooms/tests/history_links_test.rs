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

fn wrapped_url() -> String {
    format!(
        "https://example.feishu.cn/docx/SyntheticWrappedDocument?from=bus&login_token={}&view=full",
        "synthetic0123456789".repeat(12)
    )
}

/// Every displayed cell of a wrapped token, grouped by row. The following
/// word deliberately shares its last row when there is room.
fn wrapped_cells(ui: &BusUi, target: &str) -> Vec<Vec<(u16, u16)>> {
    use unicode_width::UnicodeWidthStr;
    let mut joined = String::new();
    let mut cells = Vec::new();
    for (index, line) in ui
        .history
        .cached()
        .iter()
        .enumerate()
        .filter(|(_, line)| is_reply(line))
    {
        let body = &line.text[line.copy_from..];
        let fragment = body.split(" AFTER").next().unwrap();
        if fragment == "AFTER" || fragment.is_empty() {
            continue;
        }
        joined.push_str(fragment);
        let x = ui.view.history_text.x + line.text[..line.copy_from].width() as u16;
        let y = ui.view.history_text.y + (index - ui.main_scroll) as u16;
        cells.push(
            (0..fragment.width() as u16)
                .map(|offset| (x + offset, y))
                .collect(),
        );
    }
    assert_eq!(
        joined, target,
        "the renderer split this token without changing it"
    );
    assert!(cells.len() >= 3, "fixture must span several rows");
    cells
}

#[test]
fn wrapped_url_styles_every_fragment_and_leaves_following_text_plain() {
    let (mut ui, room, agent) = fixture();
    let url = wrapped_url();
    saved_exchange(&mut ui, room, agent, "go", &format!("{url} AFTER"));
    let buffer = settled_screen(&mut ui);
    for row in wrapped_cells(&ui, &url) {
        for at in row {
            assert!(
                is_link(&buffer, at),
                "wrapped URL cell {at:?} must be a link"
            );
        }
    }
    let after = locate(&ui, "AFTER");
    assert!(!is_link(&buffer, after));
    assert_eq!(opened(&click(&mut ui, after)), None);
}

#[test]
fn wrapped_url_click_on_every_row_opens_the_whole_target() {
    let (mut ui, room, agent) = fixture();
    let url = wrapped_url();
    saved_exchange(&mut ui, room, agent, "go", &format!("{url} AFTER"));
    settled_screen(&mut ui);
    for row in wrapped_cells(&ui, &url) {
        assert_eq!(
            opened(&click(&mut ui, row[row.len() / 2])),
            Some(url.clone())
        );
    }
}

#[test]
fn wrapped_existing_path_styles_and_opens_every_fragment() {
    let dir = scratch("wrapped");
    let doc = dir.join(format!("{}.md", "synthetic-document-".repeat(11)));
    std::fs::write(&doc, "wrapped path").unwrap();
    let target = doc.display().to_string();
    let (mut ui, room, agent) = fixture();
    saved_exchange(&mut ui, room, agent, "go", &format!("`{target}` AFTER"));
    let buffer = settled_screen(&mut ui);
    for row in wrapped_cells(&ui, &target) {
        for &at in &row {
            assert!(
                is_link(&buffer, at),
                "wrapped path cell {at:?} must be a link"
            );
        }
        assert_eq!(
            opened(&click(&mut ui, row[row.len() / 2])),
            Some(target.clone())
        );
    }
    let after = locate(&ui, "AFTER");
    assert!(!is_link(&buffer, after));
    assert_eq!(opened(&click(&mut ui, after)), None);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn wrapped_links_keep_targets_in_prompts_code_lists_and_after_resize() {
    let url = wrapped_url();
    for body in [
        format!("{url} AFTER"),
        format!("**See** `{url}` AFTER"),
        format!("```text\n{url} AFTER\n```"),
        format!("- {url} AFTER"),
    ] {
        let (mut ui, room, agent) = fixture();
        saved_exchange(&mut ui, room, agent, &body, &body);
        for width in [120, 100] {
            ui.compute_view(width, 40);
            let mut buffer =
                ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, width, 40));
            ui.render(&mut buffer);
            let hits = ui.view.history_links.clone();
            assert!(hits.len() >= 6, "prompt and reply both wrap: {body}");
            for (rect, target) in hits {
                assert_eq!(target, links::Target::Url(url.clone()));
                assert!(is_link(&buffer, (rect.x, rect.y)));
                assert!(is_link(&buffer, (rect.right() - 1, rect.y)));
                assert_eq!(opened(&click(&mut ui, (rect.x, rect.y))), Some(url.clone()));
            }
        }
    }
}

#[test]
fn wrapped_links_do_not_join_hard_lines_or_same_target_occurrences() {
    let url = wrapped_url();
    let (mut ui, room, agent) = fixture();
    saved_exchange(
        &mut ui,
        room,
        agent,
        "go",
        &format!("https://example.test/short\n\nAFTER\n\n{url} AFTER {url} AFTER"),
    );
    let buffer = settled_screen(&mut ui);
    let short = locate(&ui, "https://example.test/short");
    assert_eq!(
        opened(&click(&mut ui, short)),
        Some("https://example.test/short".into())
    );
    for (index, line) in ui
        .history
        .cached()
        .iter()
        .enumerate()
        .filter(|(_, line)| is_reply(line))
    {
        for (offset, _) in line.text.match_indices("AFTER") {
            use unicode_width::UnicodeWidthStr;
            let at = (
                ui.view.history_text.x + line.text[..offset].width() as u16,
                ui.view.history_text.y + (index - ui.main_scroll) as u16,
            );
            assert!(!is_link(&buffer, at), "following text must stay plain");
            assert!(ui
                .view
                .history_links
                .iter()
                .all(|(rect, _)| !rect.contains(at.into())));
        }
    }
}

#[test]
fn wrapped_quoted_path_includes_spaces_and_keeps_the_author_base() {
    let dir = scratch("quoted-wrap");
    let name = format!("{}report with space.txt", "synthetic-folder-".repeat(10));
    let path = dir.join(&name);
    std::fs::write(&path, "quoted wrapped path").unwrap();
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
        "go",
        &format!("\"./{name}:12:3\" AFTER"),
    );
    let buffer = settled_screen(&mut ui);
    let expected = dir.join(format!("./{name}"));
    let hits = ui.view.history_links.clone();
    assert!(hits.len() >= 3);
    for (rect, target) in hits {
        assert_eq!(target, links::Target::Path(expected.clone()));
        for x in rect.x..rect.right() {
            assert!(is_link(&buffer, (x, rect.y)));
        }
        assert_eq!(
            opened(&click(&mut ui, (rect.x, rect.y))),
            Some(expected.display().to_string())
        );
    }
    let after = locate(&ui, "AFTER");
    assert!(!is_link(&buffer, after));
    assert_eq!(opened(&click(&mut ui, after)), None);
    std::fs::remove_dir_all(dir).unwrap();
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

#[test]
fn dragging_an_attachment_name_copies_without_opening() {
    let dir = scratch("attachment-drag");
    let doc = dir.join("notes.md");
    std::fs::write(&doc, "notes").unwrap();
    let (mut ui, room, agent) = fixture();
    exchange_with_files(&mut ui, room, agent, std::slice::from_ref(&doc));
    settled_screen(&mut ui);
    let (x, y) = locate(&ui, "notes.md");
    let mut all = actions(&mut ui, MouseEventKind::Down(Left), x, y);
    all.extend(actions(&mut ui, MouseEventKind::Drag(Left), x + 7, y));
    all.extend(actions(&mut ui, MouseEventKind::Up(Left), x + 7, y));
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(opened(&all), None, "dragging must not open the attachment");
    let copied = all.iter().find_map(|action| match action {
        ClientShellAction::ClipboardWrite(bytes) => String::from_utf8(bytes.clone()).ok(),
        _ => None,
    });
    assert_eq!(copied.as_deref(), Some("notes.md"));
}

#[test]
fn releasing_over_a_link_after_pressing_other_text_does_not_open_it() {
    let (mut ui, room, agent) = fixture();
    saved_exchange(
        &mut ui,
        room,
        agent,
        "go",
        "before https://example.test after",
    );
    settled_screen(&mut ui);
    let (x, y) = locate(&ui, "before ");
    let end = locate(&ui, "https://example.test");
    let mut all = actions(&mut ui, MouseEventKind::Down(Left), x, y);
    all.extend(actions(&mut ui, MouseEventKind::Up(Left), end.0, end.1));
    assert_eq!(
        opened(&all),
        None,
        "a different release cell is not a plain click"
    );
}

#[test]
fn clicking_blank_space_beside_an_attachment_name_does_not_open_it() {
    let dir = scratch("attachment-blank");
    let doc = dir.join("notes.md");
    std::fs::write(&doc, "notes").unwrap();
    let (mut ui, room, agent) = fixture();
    exchange_with_files(&mut ui, room, agent, std::slice::from_ref(&doc));
    settled_screen(&mut ui);
    let (x, y) = locate(&ui, "notes.md");
    let all = click(&mut ui, (x + 12, y));
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(
        opened(&all),
        None,
        "blank history cells are not an attachment link"
    );
}

#[test]
fn a_picture_opens_from_its_own_cells_only() {
    let dir = thumbnail_dir("picture-link");
    let image = png(&dir, "shot.png", (200, 80));
    let (mut ui, room, agent) = fixture();
    exchange_with_files(&mut ui, room, agent, std::slice::from_ref(&image));
    ui.graphics = Some(thumbnails::Protocol::Iterm2);
    ui.thumbnails.set_protocol(thumbnails::Protocol::Iterm2);
    ui.thumbnails.set_cell(Some(CELL));
    ui.compute_view(100, 40);
    let picture = ui.view.thumbnails[0].clone();
    let inside = (picture.x + picture.cols - 1, picture.y + picture.rows - 1);
    let beside = (picture.x + picture.cols + 2, picture.y);
    assert_eq!(opened(&click(&mut ui, beside)), None, "beside the picture");
    assert_eq!(
        opened(&click(&mut ui, inside)),
        Some(image.display().to_string())
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dragging_within_a_picture_does_not_open_it() {
    let dir = thumbnail_dir("picture-drag-link");
    let image = png(&dir, "shot.png", (200, 80));
    let (mut ui, room, agent) = fixture();
    exchange_with_files(&mut ui, room, agent, std::slice::from_ref(&image));
    ui.graphics = Some(thumbnails::Protocol::Iterm2);
    ui.thumbnails.set_protocol(thumbnails::Protocol::Iterm2);
    ui.thumbnails.set_cell(Some(CELL));
    ui.compute_view(100, 40);
    let picture = ui.view.thumbnails[0].clone();
    assert!(picture.cols > 3);
    let mut all = actions(&mut ui, MouseEventKind::Down(Left), picture.x, picture.y);
    all.extend(actions(
        &mut ui,
        MouseEventKind::Drag(Left),
        picture.x + 3,
        picture.y,
    ));
    all.extend(actions(
        &mut ui,
        MouseEventKind::Up(Left),
        picture.x + 3,
        picture.y,
    ));
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(opened(&all), None, "a thumbnail drag is not a click");
}

/// Every reply row that shows part of the wrapped URL has a hit, every hit
/// opens the complete URL, and the following word stays plain.
fn assert_wrapped_reply_url_links_on_every_row(body: &str) {
    let (mut ui, room, agent) = fixture();
    let url = wrapped_url();
    saved_exchange(&mut ui, room, agent, "go", &body.replace("{url}", &url));
    let buffer = settled_screen(&mut ui);
    let hits = ui.view.history_links.clone();
    let mut rows = 0;
    for (index, line) in ui
        .history
        .cached()
        .iter()
        .enumerate()
        .filter(|(_, line)| is_reply(line))
    {
        if !(line.text.contains("https://example.feishu") || line.text.contains("synthetic")) {
            continue;
        }
        rows += 1;
        let y = ui.view.history_text.y + (index - ui.main_scroll) as u16;
        assert!(
            hits.iter().any(|(rect, _)| rect.y == y),
            "{body}: row {:?} has no link",
            line.text
        );
    }
    assert!(rows >= 3, "{body}: fixture must wrap over several rows");
    for (rect, target) in hits {
        assert_eq!(target, links::Target::Url(url.clone()), "{body}");
        assert!(is_link(&buffer, (rect.x, rect.y)), "{body}");
        assert_eq!(opened(&click(&mut ui, (rect.x, rect.y))), Some(url.clone()));
    }
    let after = locate(&ui, "AFTER");
    assert!(!is_link(&buffer, after), "{body}: following text is plain");
}

#[test]
fn wrapped_url_after_cjk_text_and_fullwidth_colon_links_every_row() {
    assert_wrapped_reply_url_links_on_every_row("请打开飞书文档链接：{url} AFTER");
}

#[test]
fn wrapped_url_in_a_block_quote_links_every_row() {
    assert_wrapped_reply_url_links_on_every_row("> {url} AFTER");
}

#[test]
fn wrapped_url_in_a_numbered_list_links_every_row() {
    assert_wrapped_reply_url_links_on_every_row("1. 文档\n2. {url} AFTER");
}

#[test]
fn wrapped_url_after_a_markdown_link_to_the_same_target_links_every_row() {
    assert_wrapped_reply_url_links_on_every_row("见[文档]({url})，原链接 {url} AFTER");
}

#[test]
fn wrapped_url_in_a_stacked_table_cell_links_every_row() {
    assert_wrapped_reply_url_links_on_every_row(
        "| 名称 | 链接 |\n|---|---|\n| 文档 | {url} |\n\nAFTER",
    );
}

#[test]
fn wrapped_url_label_of_a_markdown_link_links_every_row() {
    assert_wrapped_reply_url_links_on_every_row("[{url}]({url}) AFTER");
}
