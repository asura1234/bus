use super::terminal::write_numbered_lines;
use super::*;
use crate::protocol::kitty::placement::KittyImageFormat;

#[test]
fn kitty_image_fingerprint_covers_full_payload() {
    let mut data = vec![1u8; 4096 * 4];
    let original =
        kitty_image_fingerprint(data.as_ptr(), data.len(), 100, 50, KittyImageFormat::Png);

    data[4096 + 123] = 2;
    let changed_outside_sampled_windows =
        kitty_image_fingerprint(data.as_ptr(), data.len(), 100, 50, KittyImageFormat::Png);
    assert_ne!(original, changed_outside_sampled_windows);
}

#[test]
fn kitty_image_fingerprint_refreshes_on_retransmission() {
    let mut terminal = Terminal::new(10, 5, 0).unwrap();
    terminal.write(b"\x1b_Ga=T,f=32,t=d,i=7,p=3,s=1,v=1,c=10,r=5,q=2;/wAA/w==\x1b\\");
    let first = terminal
        .kitty_image_placements_with_data_filter(|_| true)
        .unwrap();
    assert_eq!(first.len(), 1);
    let first_generation = terminal
        .kitty_fingerprints
        .lock()
        .unwrap()
        .get(&7)
        .unwrap()
        .generation;
    assert_ne!(first_generation, 0);

    // Same id and size, different pixels.
    terminal.write(b"\x1b_Ga=t,f=32,t=d,i=7,s=1,v=1,q=2;AAAAAA==\x1b\\");
    let second = terminal
        .kitty_image_placements_with_data_filter(|_| true)
        .unwrap();
    assert_eq!(second.len(), 1);
    assert_ne!(first[0].data_fingerprint, second[0].data_fingerprint);
    let second_generation = terminal
        .kitty_fingerprints
        .lock()
        .unwrap()
        .get(&7)
        .unwrap()
        .generation;
    assert_ne!(first_generation, second_generation);

    // No retransmission, so the fingerprint and generation stay stable.
    let third = terminal
        .kitty_image_placements_with_data_filter(|_| true)
        .unwrap();
    assert_eq!(second[0].data_fingerprint, third[0].data_fingerprint);
    assert_eq!(
        terminal
            .kitty_fingerprints
            .lock()
            .unwrap()
            .get(&7)
            .unwrap()
            .generation,
        second_generation
    );
}

#[test]
fn kitty_storage_generation_skips_only_proven_empty_storage() {
    let mut terminal = Terminal::new(10, 5, 1_000_000).unwrap();
    terminal.enable_kitty_graphics().unwrap();
    terminal.resize(10, 5, 8, 16).unwrap();

    assert_eq!(terminal.kitty_graphics_generation().unwrap(), 0);
    assert!(!terminal.kitty_graphics_may_have_placements().unwrap());
    assert!(terminal
        .kitty_image_placements_with_data_filter(|_| true)
        .unwrap()
        .is_empty());

    terminal.write(b"\x1b_Ga=t,t=d,f=24,i=1,s=1,v=2;////////\x1b\\");
    let transmitted = terminal.kitty_graphics_generation().unwrap();
    assert_ne!(transmitted, 0);
    assert!(terminal.kitty_graphics_may_have_placements().unwrap());
    assert!(terminal
        .kitty_image_placements_with_data_filter(|_| true)
        .unwrap()
        .is_empty());
    assert_eq!(terminal.kitty_empty_generation.get(), Some(transmitted));
    assert!(!terminal.kitty_graphics_may_have_placements().unwrap());

    terminal.write(b"plain text");
    assert_eq!(terminal.kitty_graphics_generation().unwrap(), transmitted);
    assert!(terminal
        .kitty_image_placements_with_data_filter(|_| true)
        .unwrap()
        .is_empty());

    terminal.write(b"\x1b_Ga=p,i=1,p=1,c=1,r=1;\x1b\\");
    let placed = terminal.kitty_graphics_generation().unwrap();
    assert_ne!(placed, transmitted);
    assert!(terminal.kitty_graphics_may_have_placements().unwrap());
    assert_eq!(
        terminal
            .kitty_image_placements_with_data_filter(|_| true)
            .unwrap()
            .len(),
        1
    );

    terminal.resize(10, 5, 12, 24).unwrap();
    assert_eq!(terminal.kitty_graphics_generation().unwrap(), placed);
    assert_eq!(
        terminal
            .kitty_image_placements_with_data_filter(|_| true)
            .unwrap()
            .len(),
        1
    );

    write_numbered_lines(&mut terminal, 20);
    assert_eq!(terminal.kitty_graphics_generation().unwrap(), placed);
    assert!(terminal
        .kitty_image_placements_with_data_filter(|_| true)
        .unwrap()
        .is_empty());
    assert_ne!(terminal.kitty_empty_generation.get(), Some(placed));
    assert!(terminal.kitty_graphics_may_have_placements().unwrap());
    terminal.scroll_viewport_row(0);
    assert_eq!(
        terminal
            .kitty_image_placements_with_data_filter(|_| true)
            .unwrap()
            .len(),
        1
    );

    terminal.write(b"\x1b_Ga=d,d=A\x1b\\");
    let deleted = terminal.kitty_graphics_generation().unwrap();
    assert_ne!(deleted, placed);
    assert!(terminal.kitty_graphics_may_have_placements().unwrap());
    assert!(terminal
        .kitty_image_placements_with_data_filter(|_| true)
        .unwrap()
        .is_empty());
    assert_eq!(terminal.kitty_empty_generation.get(), Some(deleted));
    assert!(!terminal.kitty_graphics_may_have_placements().unwrap());
}

#[test]
fn kitty_graphics_direct_rgba_placement_is_queryable() {
    let mut terminal = Terminal::new(10, 5, 0).unwrap();
    terminal.enable_kitty_graphics().unwrap();
    terminal.resize(10, 5, 8, 16).unwrap();
    terminal.write(b"\x1b_Ga=T,f=32,t=d,i=7,p=3,s=1,v=1,c=10,r=5,q=2;/wAA/w==\x1b\\");

    let placements = terminal
        .kitty_image_placements_with_data_filter(|_| true)
        .unwrap();
    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].image_id, 7);
    assert_eq!(placements[0].placement_id, 3);
    assert_eq!(placements[0].image_width, 1);
    assert_eq!(placements[0].image_height, 1);
    assert_eq!(placements[0].format, KittyImageFormat::Rgba);
    assert_eq!(placements[0].data, [255, 0, 0, 255]);
    assert_eq!(placements[0].render.grid_cols, 10);
    assert_eq!(placements[0].render.grid_rows, 5);
}

#[test]
fn kitty_graphics_local_media_are_enabled() {
    let mut terminal = Terminal::new(10, 5, 0).unwrap();
    terminal.enable_kitty_graphics().unwrap();

    assert!(terminal
        .get_bool(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_KITTY_IMAGE_MEDIUM_FILE)
        .unwrap());
    assert!(terminal
        .get_bool(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_KITTY_IMAGE_MEDIUM_TEMP_FILE)
        .unwrap());
    assert!(terminal
        .get_bool(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_KITTY_IMAGE_MEDIUM_SHARED_MEM)
        .unwrap());
}

#[test]
fn kitty_graphics_file_medium_rgba_placement_is_queryable() {
    use base64::Engine;

    let dir = std::env::temp_dir().join(format!(
        "herdr-kitty-file-medium-test-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("pixel.rgba");
    std::fs::write(&path, [255, 0, 0, 255]).unwrap();

    let mut terminal = Terminal::new(10, 5, 0).unwrap();
    terminal.enable_kitty_graphics().unwrap();
    terminal.resize(10, 5, 8, 16).unwrap();
    let encoded_path =
        base64::engine::general_purpose::STANDARD.encode(path.as_os_str().as_encoded_bytes());
    let command = format!("\x1b_Ga=T,f=32,t=f,i=9,p=4,s=1,v=1,c=10,r=5,q=2;{encoded_path}\x1b\\");
    terminal.write(command.as_bytes());
    terminal.write(b"\x1b_Ga=p,U=1,i=9,c=10,r=5\x1b\\");

    let placements = terminal
        .kitty_image_placements_with_data_filter(|_| true)
        .unwrap();
    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].image_id, 9);
    assert_eq!(placements[0].placement_id, 4);
    assert_eq!(placements[0].image_width, 1);
    assert_eq!(placements[0].image_height, 1);
    assert_eq!(placements[0].format, KittyImageFormat::Rgba);
    assert_eq!(placements[0].data, [255, 0, 0, 255]);
    assert_eq!(placements[0].render.grid_cols, 10);
    assert_eq!(placements[0].render.grid_rows, 5);

    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[test]
fn kitty_graphics_file_upload_can_be_placed_later() {
    let dir = std::env::temp_dir().join(format!(
        "herdr-kitty-file-upload-test-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("pixel.rgba");
    std::fs::write(&path, [255, 0, 0, 255]).unwrap();

    let mut terminal = Terminal::new(10, 5, 0).unwrap();
    terminal.enable_kitty_graphics().unwrap();
    terminal.resize(10, 5, 8, 16).unwrap();
    let mut upload = Vec::new();
    let payload = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(path.to_str().unwrap().as_bytes())
    };
    upload.extend_from_slice(
        format!("\x1b_Ga=t,f=32,s=1,v=1,i=10,q=0,t=f;{payload}\x1b\\").as_bytes(),
    );
    terminal.write(&upload);
    assert!(terminal
        .kitty_image_placements_with_data_filter(|_| true)
        .unwrap()
        .is_empty());

    terminal.write(b"\x1b_Ga=p,i=10,p=5,c=10,r=5,C=1,q=2\x1b\\");
    let placements = terminal
        .kitty_image_placements_with_data_filter(|_| true)
        .unwrap();
    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].image_id, 10);
    assert_eq!(placements[0].placement_id, 5);
    assert_eq!(placements[0].image_width, 1);
    assert_eq!(placements[0].image_height, 1);
    assert_eq!(placements[0].format, KittyImageFormat::Rgba);
    assert_eq!(placements[0].data, [255, 0, 0, 255]);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn kitty_graphics_unicode_placeholder_placement_is_queryable() {
    let mut terminal = Terminal::new(10, 5, 0).unwrap();
    terminal.enable_kitty_graphics().unwrap();
    terminal.resize(10, 5, 8, 16).unwrap();
    terminal.write(b"\x1b_Gq=2,a=t,t=d,f=32,s=1,v=1,i=1193046,m=0;/wAA/w==\x1b\\");
    terminal.write(b"\x1b_Gq=2,a=p,U=1,i=1193046,c=2,r=1\x1b\\");
    terminal.write(
        "\x1b[2;3H\x1b[38;2;18;52;86m\u{10eeee}\u{0305}\u{0305}\u{10eeee}\u{0305}\u{030d}\x1b[0m"
            .as_bytes(),
    );

    let placements = terminal
        .kitty_image_placements_with_data_filter(|_| true)
        .unwrap();
    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].image_id, 1193046);
    assert_ne!(placements[0].placement_id, 0);
    assert_eq!(placements[0].image_width, 1);
    assert_eq!(placements[0].image_height, 1);
    assert_eq!(placements[0].format, KittyImageFormat::Rgba);
    assert_eq!(placements[0].data, [255, 0, 0, 255]);
    assert_eq!(placements[0].render.viewport_col, 2);
    assert_eq!(placements[0].render.viewport_row, 1);
    assert_eq!(placements[0].render.grid_cols, 2);
    assert_eq!(placements[0].render.grid_rows, 1);
}
