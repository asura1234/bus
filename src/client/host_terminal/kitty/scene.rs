//! Client-owned desired scene, resident pixels, host uploads and cleanup.
use super::{encode_graphics_update_incremental, HostGraphicsCache};
use crate::protocol::kitty::apc::encode_delete_image;
use crate::protocol::kitty::placement::{
    HostCellSize, HostPlacement, HostSourceKey, ImageSignature, KittyImageFormat,
    KittyImagePlacement, KittyPlacementRenderInfo,
};
use crate::protocol::wire::SurfaceGraphicsVisibility as Visibility;
use crate::protocol::{
    SurfaceGraphicsAssetKey, SurfaceGraphicsFormat, SurfaceGraphicsPlacement, SurfaceGraphicsScene,
    SurfaceGraphicsSource, SurfaceGraphicsTarget,
};
use crate::utils::ids::PaneId;
use ratatui::layout::Rect;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

#[derive(Debug, Default)]
pub(crate) struct ClientState {
    scope: String,
    scene: SurfaceGraphicsScene,
    assets: HashMap<SurfaceGraphicsAssetKey, Vec<u8>>,
    host: HostGraphicsCache,
    reset_pending: bool,
    stale_images: Vec<u32>,
    forced_delete_images: Vec<u32>,
}

impl ClientState {
    pub(crate) fn scope(&self) -> &str {
        &self.scope
    }

    pub(crate) fn set_scope(&mut self, scope: &str) {
        if self.scope == scope {
            return;
        }
        self.scope = scope.to_owned();
        self.scene = SurfaceGraphicsScene::default();
        self.assets.clear();
        self.stale_images.clear();
        self.forced_delete_images.clear();
        self.reset_pending = true;
    }

    pub(crate) fn take_pending_cleanup(&mut self) -> Vec<u8> {
        let mut bytes = if self.reset_pending {
            self.reset_pending = false;
            self.stale_images.clear();
            self.host.clear_bytes()
        } else {
            Vec::new()
        };
        self.forced_delete_images.sort_unstable();
        self.forced_delete_images.dedup();
        for image_id in self.forced_delete_images.drain(..) {
            self.host.images.remove(&image_id);
            self.host.placements.retain(|(id, _), _| *id != image_id);
            self.host.sources.retain(|_, id| *id != image_id);
            self.host
                .replayed_placements
                .retain(|(id, _)| *id != image_id);
            encode_delete_image(&mut bytes, image_id);
        }
        bytes
    }

    pub(crate) fn set_scene(&mut self, mut scene: SurfaceGraphicsScene) {
        let desired = scene
            .placements
            .iter()
            .map(|placement| placement.asset.clone())
            .collect::<HashSet<_>>();
        let previous = self
            .scene
            .placements
            .iter()
            .map(|placement| placement.asset.clone())
            .collect::<HashSet<_>>();
        self.stale_images.extend(
            previous
                .difference(&desired)
                .map(|key| host_image_id(&self.scope, key)),
        );
        let placed = scene
            .placements
            .iter()
            .map(|placement| placement.asset.clone())
            .collect::<HashSet<_>>();
        self.assets.retain(|key, _| placed.contains(key));
        for asset in std::mem::take(&mut scene.assets) {
            if asset.data.len() as u64 == asset.key.data_len && placed.contains(&asset.key) {
                self.assets.insert(asset.key, asset.data);
            }
        }
        self.scene = scene;
    }

    pub(crate) fn encode(
        &mut self,
        visibility: Visibility,
        main_origin: (u16, u16),
        cell_size: HostCellSize,
    ) -> Vec<u8> {
        let mut bytes = self.take_pending_cleanup();
        self.stale_images.sort_unstable();
        self.stale_images.dedup();
        for image_id in self.stale_images.drain(..) {
            if self.host.images.remove(&image_id).is_some() {
                encode_delete_image(&mut bytes, image_id);
            }
            self.host.placements.retain(|(id, _), _| *id != image_id);
            self.host.sources.retain(|_, id| *id != image_id);
            self.host
                .replayed_placements
                .retain(|(id, _)| *id != image_id);
        }
        if !cell_size.is_known() || self.scope.is_empty() {
            bytes.extend(self.host.clear_bytes());
            return bytes;
        }

        let placements = self
            .scene
            .placements
            .iter()
            .filter_map(|placement| {
                client_host_placement(
                    &self.scope,
                    placement,
                    self.assets.get(&placement.asset).map(Vec::as_slice),
                    visibility,
                    main_origin,
                    cell_size,
                )
            })
            .collect::<Vec<_>>();
        self.host.request_placement_replay();
        loop {
            let encoded =
                encode_graphics_update_incremental(&mut self.host, &placements, None, false);
            bytes.extend(encoded.bytes);
            if !encoded.incomplete {
                return bytes;
            }
        }
    }
}

pub(crate) fn host_image_id(scope: &str, key: &SurfaceGraphicsAssetKey) -> u32 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    scope.hash(&mut hasher);
    key.hash(&mut hasher);
    10_000 + ((hasher.finish() as u32) % 900_000)
}

fn client_host_placement(
    scope: &str,
    placement: &SurfaceGraphicsPlacement,
    data: Option<&[u8]>,
    visibility: Visibility,
    main_origin: (u16, u16),
    cell_size: HostCellSize,
) -> Option<HostPlacement> {
    let origin = match (&placement.asset.source, visibility) {
        (
            SurfaceGraphicsSource::Terminal {
                target: SurfaceGraphicsTarget::Pane { .. },
                ..
            },
            Visibility::Main,
        ) => main_origin,
        _ => return None,
    };
    let source_key = HostSourceKey::ClientSurface {
        scope: scope.to_owned(),
        source: placement.asset.source.clone(),
    };
    let signature = ImageSignature {
        image_width: placement.asset.image_width,
        image_height: placement.asset.image_height,
        format_code: format_code(placement.asset.format),
        data_len: usize::try_from(placement.asset.data_len).unwrap_or(usize::MAX),
        data_fingerprint: placement.asset.data_fingerprint,
    };
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    scope.hash(&mut hasher);
    placement.asset.source.hash(&mut hasher);
    signature.hash(&mut hasher);
    let raw = hasher.finish();
    let pane_id = PaneId::from_raw((raw as u32).max(1));
    let host_image_id = host_image_id(scope, &placement.asset);
    let cols = placement.cols.min(u32::from(u16::MAX)) as u16;
    let rows = placement.rows.min(u32::from(u16::MAX)) as u16;
    Some(HostPlacement {
        pane_id,
        host_image_id: Some(host_image_id),
        area: Rect::new(
            origin.0.saturating_add(placement.x),
            origin.1.saturating_add(placement.y),
            cols,
            rows,
        ),
        cell_size,
        source_key,
        placement: KittyImagePlacement {
            image_id: 1,
            placement_id: placement.logical_placement_id,
            z: placement.z,
            x_offset: placement.x_offset,
            y_offset: placement.y_offset,
            image_width: placement.asset.image_width,
            image_height: placement.asset.image_height,
            format: match placement.asset.format {
                SurfaceGraphicsFormat::Rgb => KittyImageFormat::Rgb,
                SurfaceGraphicsFormat::Rgba => KittyImageFormat::Rgba,
                SurfaceGraphicsFormat::Png => KittyImageFormat::Png,
            },
            data_len: usize::try_from(placement.asset.data_len).unwrap_or(usize::MAX),
            data_fingerprint: placement.asset.data_fingerprint,
            data: data.unwrap_or_default().to_vec(),
            render: KittyPlacementRenderInfo {
                pixel_width: placement.cols.saturating_mul(cell_size.width_px),
                pixel_height: placement.rows.saturating_mul(cell_size.height_px),
                grid_cols: placement.cols,
                grid_rows: placement.rows,
                viewport_col: 0,
                viewport_row: 0,
                source_x: placement.source_x,
                source_y: placement.source_y,
                source_width: placement.source_width,
                source_height: placement.source_height,
            },
        },
        scrollback_offset: placement.scrollback_offset,
    })
}

fn format_code(format: SurfaceGraphicsFormat) -> u32 {
    match format {
        SurfaceGraphicsFormat::Rgb => 24,
        SurfaceGraphicsFormat::Rgba => 32,
        SurfaceGraphicsFormat::Png => 100,
    }
}
