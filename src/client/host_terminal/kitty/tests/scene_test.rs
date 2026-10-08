use crate::client::host_terminal::kitty::scene::ClientState;
use crate::protocol::kitty::placement::HostCellSize;
use crate::protocol::wire::SurfaceGraphicsVisibility as Visibility;
use crate::protocol::{
    SurfaceGraphicsAsset, SurfaceGraphicsAssetKey, SurfaceGraphicsFormat, SurfaceGraphicsPlacement,
    SurfaceGraphicsScene, SurfaceGraphicsSource, SurfaceGraphicsTarget,
};

fn asset(target: SurfaceGraphicsTarget, fingerprint: u64, data: Vec<u8>) -> SurfaceGraphicsAsset {
    SurfaceGraphicsAsset {
        key: SurfaceGraphicsAssetKey {
            source: SurfaceGraphicsSource::Terminal {
                target,
                image_id: 7,
            },
            image_width: 1,
            image_height: 1,
            format: SurfaceGraphicsFormat::Rgba,
            data_len: data.len() as u64,
            data_fingerprint: fingerprint,
        },
        data,
    }
}

fn scene(asset: SurfaceGraphicsAsset, x: u16, y: u16) -> SurfaceGraphicsScene {
    SurfaceGraphicsScene {
        placements: vec![SurfaceGraphicsPlacement {
            asset: asset.key.clone(),
            logical_placement_id: 3,
            x,
            y,
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
        }],
        assets: vec![asset],
    }
}

#[test]
fn client_encodes_final_main_origin_upload_once_and_replays_placement() {
    let mut state = ClientState::default();
    state.set_scope("endpoint-a:boot-1");
    let image = asset(
        SurfaceGraphicsTarget::Pane {
            pane_id: "w1:p1".into(),
        },
        11,
        vec![1, 2, 3, 4],
    );
    state.set_scene(scene(image, 1, 2));

    let first = state.encode(
        Visibility::Main,
        (10, 5),
        HostCellSize {
            width_px: 8,
            height_px: 16,
        },
    );
    assert!(String::from_utf8_lossy(&first).contains("a=t,t=d"));
    assert!(String::from_utf8_lossy(&first).contains("\u{1b}[8;12H"));

    let second = state.encode(
        Visibility::Main,
        (10, 5),
        HostCellSize {
            width_px: 8,
            height_px: 16,
        },
    );
    let second = String::from_utf8_lossy(&second);
    assert!(!second.contains("a=t,t=d"));
    assert!(second.contains("a=p"));
    assert!(second.contains("\u{1b}[8;12H"));
}

#[test]
fn client_hides_and_restores_without_reuploading_pixels() {
    let mut state = ClientState::default();
    state.set_scope("endpoint-a:boot-1");
    let image = asset(
        SurfaceGraphicsTarget::Pane {
            pane_id: "w1:p1".into(),
        },
        12,
        vec![4, 3, 2, 1],
    );
    state.set_scene(scene(image, 0, 0));
    let cell = HostCellSize {
        width_px: 8,
        height_px: 16,
    };
    let _ = state.encode(Visibility::Main, (4, 2), cell);

    let hidden = state.encode(Visibility::Hidden, (4, 2), cell);
    assert!(String::from_utf8_lossy(&hidden).contains("a=d,d=i"));

    let restored = state.encode(Visibility::Main, (4, 2), cell);
    let restored = String::from_utf8_lossy(&restored);
    assert!(restored.contains("a=p"));
    assert!(!restored.contains("a=t,t=d"));
}

#[test]
fn boot_scope_cleanup_does_not_wait_for_a_coherent_surface() {
    let mut state = ClientState::default();
    state.set_scope("endpoint-a:boot-1");
    let image = asset(
        SurfaceGraphicsTarget::Pane {
            pane_id: "w1:p1".into(),
        },
        17,
        vec![1, 2, 3, 4],
    );
    state.set_scene(scene(image, 0, 0));
    let _ = state.encode(
        Visibility::Main,
        (0, 0),
        HostCellSize {
            width_px: 8,
            height_px: 16,
        },
    );

    state.set_scope("endpoint-a:boot-2");
    let cleanup = String::from_utf8(state.take_pending_cleanup()).unwrap();
    assert!(cleanup.contains("a=d,d=I"), "{cleanup}");
    assert!(state.take_pending_cleanup().is_empty());
}

#[test]
fn replacement_scene_without_repeated_asset_bytes_keeps_resident_data() {
    let mut state = ClientState::default();
    state.set_scope("endpoint-a:boot-1");
    let image = asset(
        SurfaceGraphicsTarget::Pane {
            pane_id: "w1:p1".into(),
        },
        14,
        vec![1, 1, 1, 1],
    );
    let first_scene = scene(image, 0, 0);
    let mut replacement = first_scene.clone();
    replacement.assets.clear();
    state.set_scene(first_scene);
    state.set_scene(replacement);

    let bytes = state.encode(
        Visibility::Main,
        (0, 0),
        HostCellSize {
            width_px: 8,
            height_px: 16,
        },
    );
    assert!(String::from_utf8_lossy(&bytes).contains("a=t,t=d"));
}
