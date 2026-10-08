use super::*;

fn key(fingerprint: u64) -> SurfaceGraphicsAssetKey {
    SurfaceGraphicsAssetKey {
        source: SurfaceGraphicsSource::Terminal {
            target: SurfaceGraphicsTarget::Pane {
                pane_id: "w1:p1".into(),
            },
            image_id: 7,
        },
        image_width: 1,
        image_height: 1,
        format: SurfaceGraphicsFormat::Rgba,
        data_len: 4,
        data_fingerprint: fingerprint,
    }
}

fn placement(asset: SurfaceGraphicsAssetKey) -> SurfaceGraphicsPlacement {
    SurfaceGraphicsPlacement {
        asset,
        logical_placement_id: 3,
        x: 0,
        y: 0,
        cols: 1,
        rows: 1,
        source_x: 0,
        source_y: 0,
        source_width: 1,
        source_height: 1,
        x_offset: 0,
        y_offset: 0,
        z: 0,
        scrollback_offset: 0,
    }
}

#[test]
fn delivered_pixels_are_not_repeated_and_removed_sources_leave_the_delivery_index() {
    let current = key(1);
    let retired = key(2);
    let delivered = DeliveryCache {
        assets: [current.clone(), retired].into(),
        pending: true,
    };
    let (scene, next) = package_scene_assets(
        vec![placement(current.clone())],
        [(current.clone(), vec![1, 2, 3, 4])].into(),
        &delivered,
    );
    assert!(scene.assets.is_empty());
    assert_eq!(scene.placements.len(), 1);
    assert_eq!(next.assets, HashSet::from([current]));
    assert!(!next.has_pending());
}

#[test]
fn oversized_pixels_keep_the_desired_placement_without_claiming_delivery() {
    let desired = key(3);
    let oversized = vec![0; HEADLESS_GRAPHICS_TRANSACTION_BUDGET];
    assert!(image_transfer_estimated_size(oversized.len()) > HEADLESS_GRAPHICS_TRANSACTION_BUDGET);
    let (scene, next) = package_scene_assets(
        vec![placement(desired.clone())],
        [(desired.clone(), oversized)].into(),
        &DeliveryCache::default(),
    );
    assert_eq!(scene.placements[0].asset, desired);
    assert!(scene.assets.is_empty());
    assert!(next.assets.is_empty());
    assert!(!next.has_pending());
}
