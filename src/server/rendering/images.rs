//! Server projection of VT image facts into neutral surface scenes.
use std::collections::{HashMap, HashSet};
mod visible;
use crate::protocol::kitty::placement::{
    clipped_placement, image_transfer_estimated_size, HostCellSize, HostSourceKey, ImageSignature,
    KittyImageFormat, KittyImagePlacement,
};
use crate::protocol::kitty::HEADLESS_GRAPHICS_TRANSACTION_BUDGET;
use crate::protocol::wire::{
    SurfaceGraphicsAsset, SurfaceGraphicsAssetKey, SurfaceGraphicsFormat, SurfaceGraphicsPlacement,
    SurfaceGraphicsScene, SurfaceGraphicsSource, SurfaceGraphicsTarget,
};
use visible::collect_visible_placements;
const MAX_SURFACE_GRAPHICS_PLACEMENTS: usize = 4_096;

#[derive(Clone, Debug, Default)]
pub(crate) struct DeliveryCache {
    assets: HashSet<SurfaceGraphicsAssetKey>,
    pending: bool,
}

impl DeliveryCache {
    pub(crate) fn has_pending(&self) -> bool {
        self.pending
    }
}

pub(crate) fn collect_scene(
    app: &crate::server::app::App,
    surface: crate::server::rendering::surface::TabSurfaceView<'_>,
    cell_size: HostCellSize,
    delivered: &DeliveryCache,
) -> (SurfaceGraphicsScene, DeliveryCache) {
    if !cell_size.is_known() {
        return (SurfaceGraphicsScene::default(), DeliveryCache::default());
    }
    let workspace_index = surface.target.map(|target| target.workspace_index);
    let mut targets = HashMap::new();
    let mut public_panes = HashMap::new();
    if let Some(workspace_index) = workspace_index {
        for pane in surface.pane_infos {
            if let Some(public_id) = app.public_pane_id(workspace_index, pane.id) {
                public_panes.insert(public_id.clone(), pane.id);
                targets.insert(pane.id, SurfaceGraphicsTarget::Pane { pane_id: public_id });
            }
        }
    }

    // Reconstruct only the small image-signature index expected by the existing
    // collector. This prevents copying already-delivered image payloads on each
    // pane-scaled render while keeping Ghostty as the authoritative image store.
    let uploaded_images = HashMap::new();
    let mut delivered_terminal_images = HashMap::new();
    for key in &delivered.assets {
        let signature = image_signature_from_asset(key);
        match &key.source {
            SurfaceGraphicsSource::Terminal {
                target: SurfaceGraphicsTarget::Pane { pane_id },
                image_id,
            } => {
                if let Some(pane_id) = public_panes.get(pane_id) {
                    delivered_terminal_images.insert(
                        HostSourceKey::Terminal {
                            pane_id: *pane_id,
                            image_id: *image_id,
                        },
                        signature,
                    );
                }
            }
        }
    }
    let host_placements = collect_visible_placements(
        &app.state,
        &app.terminal_runtimes,
        surface,
        cell_size,
        &uploaded_images,
        &delivered_terminal_images,
    );

    let mut placements = Vec::new();
    let mut asset_data = HashMap::<SurfaceGraphicsAssetKey, Vec<u8>>::new();
    for mut placement in host_placements {
        if placements.len() == MAX_SURFACE_GRAPHICS_PLACEMENTS {
            break;
        }
        let Some(target) = targets.get(&placement.pane_id).cloned() else {
            continue;
        };
        let source = match &placement.source_key {
            HostSourceKey::Terminal { image_id, .. } => SurfaceGraphicsSource::Terminal {
                target,
                image_id: *image_id,
            },
            HostSourceKey::ClientSurface { .. } => continue,
        };
        let Some((clipped, _)) = clipped_placement(&placement) else {
            continue;
        };
        let asset = asset_key(source, &placement.placement);
        if !placement.placement.data.is_empty() {
            asset_data
                .entry(asset.clone())
                .or_insert_with(|| std::mem::take(&mut placement.placement.data));
        }
        placements.push(SurfaceGraphicsPlacement {
            asset,
            logical_placement_id: placement.placement.placement_id,
            x: clipped.x,
            y: clipped.y,
            cols: clipped.cols,
            rows: clipped.rows,
            source_x: clipped.source_x,
            source_y: clipped.source_y,
            source_width: clipped.source_width,
            source_height: clipped.source_height,
            x_offset: clipped.x_offset,
            y_offset: clipped.y_offset,
            z: placement.placement.z,
            scrollback_offset: placement.scrollback_offset,
        });
    }

    package_scene_assets(placements, asset_data, delivered)
}

fn package_scene_assets(
    mut placements: Vec<SurfaceGraphicsPlacement>,
    asset_data: HashMap<SurfaceGraphicsAssetKey, Vec<u8>>,
    delivered: &DeliveryCache,
) -> (SurfaceGraphicsScene, DeliveryCache) {
    let desired = placements
        .iter()
        .map(|placement| placement.asset.clone())
        .collect::<HashSet<_>>();
    let mut next = DeliveryCache {
        assets: delivered.assets.intersection(&desired).cloned().collect(),
        pending: false,
    };
    let mut assets = Vec::new();
    let mut available = asset_data.into_iter().collect::<Vec<_>>();
    available.sort_by_key(|(key, _)| format!("{:?}", key.source));
    let mut payload_bytes = 0usize;
    for (key, data) in available {
        if next.assets.contains(&key) {
            continue;
        }
        let encoded_size = image_transfer_estimated_size(data.len());
        if encoded_size > HEADLESS_GRAPHICS_TRANSACTION_BUDGET {
            continue;
        }
        if payload_bytes.saturating_add(encoded_size) > HEADLESS_GRAPHICS_TRANSACTION_BUDGET {
            next.pending = true;
            continue;
        }
        payload_bytes = payload_bytes.saturating_add(encoded_size);
        assets.push(SurfaceGraphicsAsset {
            key: key.clone(),
            data,
        });
        next.assets.insert(key);
    }
    assets.sort_by_key(|asset| format!("{:?}", asset.key.source));
    placements.sort_by_key(|placement| {
        (
            format!("{:?}", placement.asset.source),
            placement.logical_placement_id,
            placement.y,
            placement.x,
        )
    });
    (SurfaceGraphicsScene { assets, placements }, next)
}

fn image_signature_from_asset(key: &SurfaceGraphicsAssetKey) -> ImageSignature {
    ImageSignature {
        image_width: key.image_width,
        image_height: key.image_height,
        format_code: format_code(key.format),
        data_len: usize::try_from(key.data_len).unwrap_or(usize::MAX),
        data_fingerprint: key.data_fingerprint,
    }
}

fn asset_key(
    source: SurfaceGraphicsSource,
    placement: &KittyImagePlacement,
) -> SurfaceGraphicsAssetKey {
    SurfaceGraphicsAssetKey {
        source,
        image_width: placement.image_width,
        image_height: placement.image_height,
        format: match placement.format {
            KittyImageFormat::Rgb => SurfaceGraphicsFormat::Rgb,
            KittyImageFormat::Rgba => SurfaceGraphicsFormat::Rgba,
            KittyImageFormat::Png => SurfaceGraphicsFormat::Png,
        },
        data_len: placement.data_len as u64,
        data_fingerprint: placement.data_fingerprint,
    }
}

pub(crate) fn collect_retained(
    app: &crate::server::app::App,
    surface: &crate::protocol::wire::PaneSurfaceFrame,
    target: crate::server::rendering::surface::TabSurfaceTarget,
    cell_size: crate::protocol::kitty::HostCellSize,
    delivered: &DeliveryCache,
) -> Option<(SurfaceGraphicsScene, DeliveryCache)> {
    let rect = |rect: crate::protocol::wire::SurfaceRect| {
        ratatui::layout::Rect::new(rect.x, rect.y, rect.width, rect.height)
    };
    let pane_infos = surface
        .panes
        .iter()
        .map(|pane| {
            let (workspace_index, id) = app.parse_pane_id(&pane.pane_id)?;
            if workspace_index != target.workspace_index {
                return None;
            }
            Some(crate::server::workspaces::layout::PaneInfo {
                id,
                rect: rect(pane.rect),
                inner_rect: rect(pane.inner_rect),
                scrollbar_rect: pane.scrollbar_rect.map(rect),
                borders: ratatui::widgets::Borders::NONE,
                is_focused: pane.focused,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    // Image clipping uses the retained content rectangles, not borders or split handles.
    Some(collect(
        app,
        &pane_infos,
        &[],
        Some(target),
        cell_size,
        delivered,
    ))
}

pub(crate) fn collect(
    app: &crate::server::app::App,
    pane_infos: &[crate::server::workspaces::layout::PaneInfo],
    split_borders: &[crate::server::workspaces::layout::SplitBorder],
    target: Option<crate::server::rendering::surface::TabSurfaceTarget>,
    cell_size: crate::protocol::kitty::HostCellSize,
    delivered: &DeliveryCache,
) -> (SurfaceGraphicsScene, DeliveryCache) {
    crate::server::rendering::images::collect_scene(
        app,
        crate::server::rendering::surface::TabSurfaceView {
            target,
            pane_infos,
            split_borders,
        },
        cell_size,
        delivered,
    )
}

fn format_code(format: SurfaceGraphicsFormat) -> u32 {
    match format {
        SurfaceGraphicsFormat::Rgb => 24,
        SurfaceGraphicsFormat::Rgba => 32,
        SurfaceGraphicsFormat::Png => 100,
    }
}

#[cfg(test)]
#[path = "tests/images_test.rs"]
mod tests;
