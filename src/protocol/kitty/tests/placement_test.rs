use super::placement::image_signature_from_descriptor;
use super::*;

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
            render: crate::ghostty::KittyPlacementRenderInfo {
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

include!("../../../client/host_terminal/tests/kitty_test.rs");
