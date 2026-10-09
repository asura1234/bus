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

#[test]
fn image_with_readable_header_but_invalid_pixels_keeps_its_filename_visible() {
    let dir = thumbnail_dir("invalid-pixels");
    let path = png(&dir, "damaged-attachment.png", (200, 80));
    let mut bytes = std::fs::read(&path).unwrap();
    // Damage the compressed pixels while retaining the image's valid header.
    let pixels = bytes.windows(4).position(|chunk| chunk == b"IDAT").unwrap() + 4;
    bytes[pixels] = 0;
    std::fs::write(&path, bytes).unwrap();
    assert_eq!(image::image_dimensions(&path).unwrap(), (200, 80));
    assert!(image::open(&path).is_err());

    let (mut ui, room, agent) = fixture();
    exchange_with_files(&mut ui, room, agent, &[path]);
    ui.thumbnails.set_cell(Some(CELL));
    ui.compute_view(100, 40);
    let graphics = ui.thumbnail_graphics();
    assert!(graphics.is_empty(), "invalid pixels cannot produce an image upload");
    let screen = room_screen(&mut ui, 100, 40);
    std::fs::remove_dir_all(&dir).unwrap();

    assert!(
        screen.contains("[damaged-attachment.png]"),
        "an undecodable attachment must remain visible through its filename"
    );
}
