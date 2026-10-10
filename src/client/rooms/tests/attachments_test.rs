#[test]
fn composer_full_height_long_file_details_stay_readable() {
    let (mut ui, room, _) = fixture();
    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert(&"long draft\n".repeat(80));
    ui.detail_path = Some(format!("/tmp/{}/required-detail-tail.md", "a".repeat(70)));
    assert!(room_screen(&mut ui, 100, 30).contains("required-detail-tail.md"));
}

#[test]
fn failed_attachment_cannot_be_erased_by_different_successful_attachment() {
    let (mut ui, room, _) = fixture();
    ui.queue(
        BusCommand::AttachFile(room, "/missing".into()),
        Effect::Files(room),
    );
    ui.queue(
        BusCommand::AttachFile(room, "/existing".into()),
        Effect::Files(room),
    );
    ui.receive_event(BusEvent::CommandFinished {
        command_id: 1,
        result: Err("missing".into()),
    });
    ui.receive_event(BusEvent::CommandFinished {
        command_id: 2,
        result: Ok(()),
    });
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.last_command_id = 2;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.settle();
    assert_eq!(ui.failed.len(), 1);
}

#[test]
fn failed_file_is_removable_and_does_not_permanently_block_later_send() {
    let (mut ui, room, _) = fixture();
    ui.queue(
        BusCommand::AttachFile(room, "/missing.md".into()),
        Effect::Files(room),
    );
    ui.receive_event(BusEvent::CommandFinished {
        command_id: 1,
        result: Err("missing".into()),
    });
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.last_command_id = 1;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.settle();
    ui.compute_view(100, 30);
    assert!(ui.view.hits.iter().any(|hit|matches!(&hit.action,render::Action::RemoveFile(path) if path.to_str()==Some("/missing.md"))));
    ui.action(render::Action::RemoveFile("/missing.md".into()));
    assert!(ui.failed.is_empty());
}

#[test]
fn overflow_mixed_file_chips_keep_full_path_inspection_and_removal() {
    use crossterm::event::MouseEventKind::{Moved, ScrollDown, ScrollUp};
    let (mut ui, room, _) = fixture();
    let paths: Vec<std::path::PathBuf> = [
        format!("/one/{}.md", "very-long-name-".repeat(10)),
        "/two/ordinary-second-attachment.md".into(),
        "/three/ordinary-third-attachment.md".into(),
        "/missing/failed-overflow-attachment.md".into(),
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    let mut snapshot = (*ui.snapshot).clone();
    for path in &paths[..3] {
        snapshot.state.attach_file(room, path.clone()).unwrap();
    }
    ui.receive_snapshot(Arc::new(snapshot));
    ui.failed.push(Pending {
        id: 50,
        command: BusCommand::AttachFile(room, paths[3].display().to_string()),
        effect: Effect::Files(room),
        enqueued: true,
        result: Some(Err("missing".into())),
    });
    ui.compute_view(100, 30);
    let next = ui
        .view
        .hits
        .iter()
        .find(|h| h.action == render::Action::ScrollFiles(true))
        .unwrap()
        .clone();
    mouse(
        &mut ui,
        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        next.rect.x,
        next.rect.y,
    );
    ui.compute_view(100, 30);
    assert!(ui
        .view
        .hits
        .iter()
        .any(|h| h.action == render::Action::RemoveFile(paths[1].clone())));
    assert!(!ui
        .view
        .hits
        .iter()
        .any(|h| h.action == render::Action::RemoveFile(paths[0].clone())));
    let previous = ui
        .view
        .hits
        .iter()
        .find(|h| h.action == render::Action::ScrollFiles(false))
        .unwrap()
        .clone();
    mouse(
        &mut ui,
        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        previous.rect.x,
        previous.rect.y,
    );
    for path in &paths {
        for _ in 0..paths.len() {
            ui.compute_view(100, 30);
            if ui
                .view
                .hits
                .iter()
                .any(|h| h.action == render::Action::RemoveFile(path.clone()))
            {
                break;
            }
            let files = ui.view.files;
            mouse(&mut ui, ScrollDown, files.x + 1, files.y);
        }
        let hit = ui
            .view
            .hits
            .iter()
            .find(|h| h.action == render::Action::RemoveFile(path.clone()))
            .expect("every successful and failed attachment must have a reachable removal target")
            .clone();
        let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
        ui.render(&mut buffer);
        let chip: String = (hit.rect.x..hit.rect.right())
            .map(|x| buffer[(x, hit.rect.y)].symbol())
            .collect();
        assert!(
            chip.contains(if path == &paths[3] { "! ×]" } else { "×]" }),
            "clipped chip retains error/removal suffix: {chip}"
        );
        mouse(&mut ui, Moved, hit.rect.x, hit.rect.y);
        assert_eq!(ui.detail_path.as_deref(), path.to_str());
        mouse(
            &mut ui,
            crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            hit.rect.x,
            hit.rect.y,
        );
        if path == &paths[3] {
            assert!(ui.failed.is_empty());
        } else {
            assert!(ui.pending.iter().any(|p| matches!(&p.command, BusCommand::RemoveFile(id, removed) if *id == room && removed == path)));
        }
        for _ in 0..paths.len() {
            let files = ui.view.files;
            mouse(&mut ui, ScrollUp, files.x + 1, files.y);
        }
    }
    assert_eq!(ui.main_scroll, 0, "file scrolling must not move replies");
}

#[test]
fn pasted_images_are_saved_once_under_the_bus_data_dir() {
    let root = std::env::temp_dir().join(format!(
        "bus-paste-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let room = RoomId(7);
    let first = input::save_pasted_image(&root, room, b"\x89PNG fake", "png").unwrap();
    let again = input::save_pasted_image(&root, room, b"\x89PNG fake", "png").unwrap();
    let other = input::save_pasted_image(&root, room, b"other", "png").unwrap();

    assert!(first.is_absolute());
    assert!(first.starts_with(root.join("attachments").join("room-7")));
    assert_eq!(first.extension().unwrap(), "png");
    assert_eq!(std::fs::read(&first).unwrap(), b"\x89PNG fake");
    assert_eq!(first, again, "one image pasted twice is one file");
    assert_ne!(first, other);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn pasted_temporary_images_are_copied_into_the_room_attachments() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp = std::env::temp_dir().join(format!("bus-temp-paste-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&temp).unwrap();
    let pasted = temp.join("pasted-image.PNG");
    std::fs::write(&pasted, b"\x89PNG pasted").unwrap();
    let notes = temp.join("notes.md");
    std::fs::write(&notes, "# notes").unwrap();
    let root = temp.join("data");
    let room = RoomId(3);

    let copy = input::copy_temporary_image(&root, room, &pasted)
        .unwrap()
        .expect("a temp image is copied");
    assert!(
        copy.starts_with(root.canonicalize().unwrap().join("attachments/room-3"))
            || copy.starts_with(root.join("attachments/room-3"))
    );
    assert_eq!(copy.extension().unwrap(), "png");
    assert_eq!(std::fs::read(&copy).unwrap(), b"\x89PNG pasted");
    assert_eq!(
        input::copy_temporary_image(&root, room, &notes).unwrap(),
        None
    );
    assert_eq!(
        input::copy_temporary_image(&root, room, &copy).unwrap(),
        None,
        "an attachment Bus already owns stays where it is"
    );
    std::fs::remove_dir_all(&temp).unwrap();
}

/// A PNG whose header (and so its dimensions) reads, but whose compressed
/// pixels fail to decode.
fn corrupt_png(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let path = png(dir, name, (200, 80));
    let mut bytes = std::fs::read(&path).unwrap();
    let pixels = bytes.windows(4).position(|chunk| chunk == b"IDAT").unwrap() + 4;
    bytes[pixels] = 0;
    std::fs::write(&path, bytes).unwrap();
    assert_eq!(image::image_dimensions(&path).unwrap(), (200, 80));
    assert!(image::open(&path).is_err());
    path
}

fn attached_paths(ui: &BusUi) -> Vec<String> {
    ui.pending
        .iter()
        .filter_map(|pending| match &pending.command {
            BusCommand::AttachFile(_, path) => Some(path.clone()),
            _ => None,
        })
        .collect()
}

fn paste(ui: &mut BusUi, text: &str) {
    ui.input(
        &RawInputEvent::Paste(text.into()),
        false,
        &mut Default::default(),
    );
}

/// Refused: nothing is attached, and a dialog names the file and the decode
/// error until Enter dismisses it.
fn assert_refused_with_dialog(ui: &mut BusUi, name: &str) {
    assert_eq!(attached_paths(ui), Vec::<String>::new(), "a corrupt image is not attached");
    let screen = room_screen(ui, 100, 30);
    assert!(
        screen.contains(&format!("Could not attach \"{name}\"")),
        "the dialog names the refused file: {screen}"
    );
    assert!(screen.contains("not a readable image"), "the dialog says why: {screen}");
    assert!(screen.contains("OK (Enter)"), "the dialog can be dismissed: {screen}");
    key(ui, KeyCode::Enter, KeyModifiers::NONE);
    let screen = room_screen(ui, 100, 30);
    assert!(!screen.contains("Could not attach"), "Enter dismisses the dialog");
    assert_eq!(attached_paths(ui), Vec::<String>::new(), "dismissing attaches nothing");
}

#[test]
fn corrupt_image_dropped_into_the_composer_is_refused_with_a_dialog() {
    let dir = thumbnail_dir("drop-corrupt");
    let path = corrupt_png(&dir, "damaged drop.png");
    let (mut ui, _, _) = fixture();
    // Terminals deliver a drop as a paste of the shell-escaped path.
    paste(&mut ui, &format!("{} ", path.display().to_string().replace(' ', "\\ ")));
    assert_refused_with_dialog(&mut ui, "damaged drop.png");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn corrupt_image_path_pasted_into_the_composer_is_refused_with_a_dialog() {
    let dir = thumbnail_dir("paste-corrupt");
    let path = corrupt_png(&dir, "damaged.png");
    let (mut ui, _, _) = fixture();
    paste(&mut ui, &path.display().to_string());
    assert_refused_with_dialog(&mut ui, "damaged.png");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn corrupt_pasted_image_data_is_refused_with_a_dialog_and_never_saved() {
    let dir = thumbnail_dir("data-corrupt");
    let bytes = std::fs::read(corrupt_png(&dir, "source.png")).unwrap();
    let root = dir.join("data");
    let (mut ui, _, _) = fixture();
    ui.attach_image_data(Some(root.clone()), &bytes, "png");
    assert_eq!(attached_paths(&ui), Vec::<String>::new());
    let screen = room_screen(&mut ui, 100, 30);
    assert!(screen.contains("Could not attach the clipboard image"), "{screen}");
    assert!(screen.contains("not a readable image"), "{screen}");
    assert!(!root.join("attachments").exists(), "a corrupt paste leaves no file behind");
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(!room_screen(&mut ui, 100, 30).contains("Could not attach"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn valid_images_attach_by_drop_pasted_path_and_pasted_data_as_before() {
    let dir = thumbnail_dir("valid-attach");
    let dropped = png(&dir, "good drop.png", (20, 10));
    let pasted = png(&dir, "good.png", (20, 10));
    let notes = dir.join("notes.png.md");
    std::fs::write(&notes, "not an image").unwrap();
    let root = dir.join("data");
    let (mut ui, _, _) = fixture();
    paste(&mut ui, &format!("{} ", dropped.display().to_string().replace(' ', "\\ ")));
    paste(&mut ui, &pasted.display().to_string());
    paste(&mut ui, &notes.display().to_string());
    ui.attach_image_data(Some(root.clone()), &std::fs::read(&pasted).unwrap(), "png");

    let attached = attached_paths(&ui);
    assert_eq!(attached.len(), 4, "{attached:?}");
    // A temp-folder image may be attached as Bus's own copy of it.
    for (path, source) in attached.iter().zip([&dropped, &pasted]) {
        assert_eq!(std::fs::read(path).unwrap(), std::fs::read(source).unwrap());
    }
    assert_eq!(attached[2], notes.display().to_string(), "non-image files are unaffected");
    assert!(std::path::Path::new(&attached[3]).starts_with(root.join("attachments")));
    assert!(!room_screen(&mut ui, 100, 30).contains("Could not attach"));
    std::fs::remove_dir_all(&dir).unwrap();
}
