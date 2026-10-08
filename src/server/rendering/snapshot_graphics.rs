use crate::protocol::SurfaceGraphicsScene;
use crate::server::rendering::images::DeliveryCache;

pub(crate) fn collect_retained(
    app: &crate::app::App,
    surface: &crate::protocol::PaneSurfaceFrame,
    target: crate::ui::TabSurfaceTarget,
    cell_size: crate::kitty_graphics::HostCellSize,
    delivered: &DeliveryCache,
) -> Option<(SurfaceGraphicsScene, DeliveryCache)> {
    let rect = |rect: crate::protocol::SurfaceRect| {
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
            Some(crate::layout::PaneInfo {
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
    app: &crate::app::App,
    pane_infos: &[crate::layout::PaneInfo],
    split_borders: &[crate::layout::SplitBorder],
    target: Option<crate::ui::TabSurfaceTarget>,
    cell_size: crate::kitty_graphics::HostCellSize,
    delivered: &DeliveryCache,
) -> (SurfaceGraphicsScene, DeliveryCache) {
    crate::server::rendering::images::collect_scene(
        app,
        crate::ui::TabSurfaceView {
            target,
            pane_infos,
            split_borders,
        },
        cell_size,
        delivered,
    )
}
