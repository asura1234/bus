use crate::protocol::kitty::placement::{
    terminal_image_needs_data, HostCellSize, HostPlacement, HostSourceKey, ImageSignature,
};
use crate::server::app_state::AppState;
use crate::terminal::TerminalRuntimeRegistry;
use std::collections::{HashMap, HashSet};

pub(crate) fn collect_visible_placements(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    surface: crate::server::rendering::surface::TabSurfaceView<'_>,
    cell_size: HostCellSize,
    uploaded_images: &HashMap<u32, ImageSignature>,
    oversized_images: &HashMap<HostSourceKey, ImageSignature>,
) -> Vec<HostPlacement> {
    let Some(target) = surface.target else {
        tracing::debug!("collect_visible_placements: no tab surface target");
        return Vec::new();
    };
    let ws_idx = target.workspace_index;
    if app
        .workspaces
        .get(ws_idx)
        .and_then(|workspace| workspace.tabs.get(target.tab_index))
        .is_none()
    {
        tracing::debug!(
            ws_idx,
            tab_idx = target.tab_index,
            "collect_visible_placements: no target tab"
        );
        return Vec::new();
    }

    tracing::debug!(
        ws_idx,
        terminal_runtimes_len = terminal_runtimes.len(),
        pane_infos_len = surface.pane_infos.len(),
        "collect_visible_placements: starting iteration"
    );
    let mut placements = Vec::new();
    for info in surface.pane_infos {
        let runtime = match app.runtime_for_pane_in_workspace(terminal_runtimes, ws_idx, info.id) {
            Some(rt) => rt,
            None => {
                tracing::debug!(pane_id = ?info.id, "collect_visible_placements: runtime not found");
                continue;
            }
        };
        let mut requested_images = HashSet::new();
        for placement in runtime.kitty_image_placements_with_data_filter(|descriptor| {
            terminal_image_needs_data(
                info.id,
                descriptor,
                uploaded_images,
                oversized_images,
                &mut requested_images,
            )
        }) {
            let scrollback_offset = runtime
                .scroll_metrics()
                .map(|m| m.offset_from_bottom as u32)
                .unwrap_or(0);
            placements.push(HostPlacement {
                pane_id: info.id,
                host_image_id: None,
                area: info.inner_rect,
                cell_size,
                source_key: HostSourceKey::Terminal {
                    pane_id: info.id,
                    image_id: placement.image_id,
                },
                placement,
                scrollback_offset,
            });
        }
    }
    tracing::debug!(
        placements_len = placements.len(),
        "collect_visible_placements: done"
    );
    placements
}
