use crate::client::host_terminal::kitty::{
    drain_graphics_updates, encode_graphics_update_incremental, HostGraphicsCache,
};
use crate::protocol::kitty::placement::{
    clipped_placement, host_placement_id, image_signature_from_descriptor,
    terminal_image_needs_data, HostCellSize, HostPlacement, HostSourceKey, KittyImageDescriptor,
    KittyImageFormat, KittyImagePlacement, KittyPlacementRenderInfo,
};
use crate::protocol::kitty::HEADLESS_GRAPHICS_TRANSACTION_BUDGET;
use crate::utils::ids::PaneId;
use ratatui::layout::Rect;
use std::collections::{hash_map::DefaultHasher, HashMap, HashSet};
use std::hash::{Hash, Hasher};

fn test_placement(viewport_col: i32, viewport_row: i32) -> HostPlacement {
    HostPlacement {
        pane_id: PaneId::from_raw(1),
        host_image_id: None,
        area: Rect::new(0, 0, 20, 10),
        cell_size: HostCellSize {
            width_px: 10,
            height_px: 10,
        },
        source_key: HostSourceKey::Terminal {
            pane_id: PaneId::from_raw(1),
            image_id: 7,
        },
        scrollback_offset: 0,
        placement: KittyImagePlacement {
            image_id: 7,
            placement_id: 3,
            z: 0,
            x_offset: 0,
            y_offset: 0,
            image_width: 30,
            image_height: 30,
            format: KittyImageFormat::Rgba,
            data_len: 30 * 30 * 4,
            data_fingerprint: 42,
            data: vec![255; 30 * 30 * 4],
            render: KittyPlacementRenderInfo {
                pixel_width: 0,
                pixel_height: 0,
                grid_cols: 3,
                grid_rows: 3,
                viewport_col,
                viewport_row,
                source_x: 0,
                source_y: 0,
                source_width: 0,
                source_height: 0,
            },
        },
    }
}

fn update(cache: &mut HostGraphicsCache, placements: &[HostPlacement], replay: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    if replay {
        cache.request_placement_replay();
    }
    bytes.extend(drain_graphics_updates(cache, placements));
    bytes
}

#[test]
fn terminal_placement_id_preserves_legacy_identity() {
    let placement = test_placement(0, 0);
    let mut legacy = DefaultHasher::new();
    placement.pane_id.raw().hash(&mut legacy);
    placement.placement.image_id.hash(&mut legacy);
    placement.placement.placement_id.hash(&mut legacy);
    let expected = 1 + ((legacy.finish() as u32) % 900_000);

    assert_eq!(
        host_placement_id(&placement.source_key, &placement.placement),
        expected
    );
}

#[test]
fn clipped_placement_handles_positive_viewport_without_wrapping() {
    let placement = test_placement(2, 2);
    let (clipped, _) = clipped_placement(&placement).expect("visible placement");

    assert_eq!(clipped.x, 2);
    assert_eq!(clipped.y, 2);
    assert_eq!(clipped.cols, 3);
    assert_eq!(clipped.rows, 3);
    assert_eq!(clipped.source_x, 0);
    assert_eq!(clipped.source_y, 0);
}

#[test]
fn clipped_placement_crops_negative_viewport_offsets() {
    let placement = test_placement(-1, -1);
    let (clipped, _) = clipped_placement(&placement).expect("partially visible placement");

    assert_eq!(clipped.x, 0);
    assert_eq!(clipped.y, 0);
    assert_eq!(clipped.cols, 2);
    assert_eq!(clipped.rows, 2);
    assert_eq!(clipped.source_x, 10);
    assert_eq!(clipped.source_y, 10);
}

include!("../../tests/kitty_test.rs");

#[test]
fn clipped_scrolled_placement_crops_after_subcell_offset() {
    use crate::protocol::kitty::apc::encode_kitty_data;
    use crate::terminal::vt::Terminal;

    let mut terminal = Terminal::new(10, 5, 1_000_000).unwrap();
    terminal.enable_kitty_graphics().unwrap();
    terminal.resize(10, 5, 10, 10).unwrap();
    let mut upload = Vec::new();
    encode_kitty_data(
        &mut upload,
        "a=T,t=d,f=32,s=30,v=30,i=7,p=3,c=3,r=3,Y=5,C=1,q=2",
        &vec![255; 30 * 30 * 4],
    );
    terminal.write(&upload);
    terminal.write(b"\x1b[5;1H\n");
    let mut placements = terminal
        .kitty_image_placements_with_data_filter(|_| true)
        .unwrap();
    assert_eq!(placements.len(), 1);
    let image = placements.remove(0);
    assert_eq!(image.render.viewport_row, -1);
    assert_eq!(image.render.pixel_height, 30);
    assert_eq!(image.y_offset, 5);

    let placement = HostPlacement {
        pane_id: PaneId::from_raw(1),
        host_image_id: None,
        area: Rect::new(0, 0, 10, 5),
        cell_size: HostCellSize {
            width_px: 10,
            height_px: 10,
        },
        source_key: HostSourceKey::Terminal {
            pane_id: PaneId::from_raw(1),
            image_id: image.image_id,
        },
        placement: image,
        scrollback_offset: 0,
    };
    let (clipped, _) = clipped_placement(&placement).expect("partially visible placement");

    // The image starts five pixels into the scrolled-away row, so only five
    // image pixels, rather than a complete ten-pixel cell, are above the view.
    assert_eq!(clipped.source_y, 5);
}

#[test]
fn clipped_placement_crops_after_left_subcell_offset() {
    let mut placement = test_placement(-1, 0);
    placement.placement.x_offset = 4;
    let (clipped, _) = clipped_placement(&placement).expect("partially visible placement");

    // Only the six image pixels past the four-pixel offset are left of the view.
    assert_eq!(clipped.source_x, 6);
    assert_eq!(clipped.x_offset, 0);
}
