use super::chrome::{apply_pane_chrome, pane_inner_rect, render_pane_borders};
use super::scrollbar::{render_pane_scrollbar, should_show_scrollbar};
use crate::app::AppState;
use crate::layout::PaneInfo;
use crate::terminal::{TerminalRuntime, TerminalRuntimeRegistry};
use ratatui::{layout::Rect, widgets::Borders, Frame};

pub(crate) fn pane_is_scrolled_back(rt: &TerminalRuntime) -> bool {
    rt.scroll_metrics()
        .is_some_and(|metrics| metrics.offset_from_bottom > 0)
}

// Full view computation reaches this helper for active and background panes.
// Keep terminal queries narrow, allocation-free, and short under the core lock.
fn terminal_inner_rect(rt: &TerminalRuntime, pane_inner: Rect, pane_scrollbars: bool) -> Rect {
    if !pane_scrollbars || pane_inner.width <= 4 || rt.alternate_screen_active() {
        return pane_inner;
    }

    Rect::new(
        pane_inner.x,
        pane_inner.y,
        pane_inner.width.saturating_sub(1),
        pane_inner.height,
    )
}

fn runtime_for_tab_pane<'a>(
    _app: &'a AppState,
    terminal_runtimes: &'a TerminalRuntimeRegistry,
    _workspace_index: usize,
    tab: &'a crate::workspace::Tab,
    pane_id: crate::layout::PaneId,
) -> Option<(&'a crate::terminal::TerminalId, &'a TerminalRuntime)> {
    let terminal_id = tab.terminal_id(pane_id)?;
    #[cfg(test)]
    if let Some(runtime) = _app
        .workspaces
        .get(_workspace_index)?
        .test_runtimes
        .get(&pane_id)
        .or_else(|| tab.runtimes.get(&pane_id))
    {
        return Some((terminal_id, runtime));
    }
    terminal_runtimes
        .get(terminal_id)
        .map(|runtime| (terminal_id, runtime))
}

fn stable_scrollbar_gutter(
    rt: &TerminalRuntime,
    pane_inner: Rect,
    pane_scrollbars: bool,
) -> (Rect, Option<Rect>) {
    let inner_rect = terminal_inner_rect(rt, pane_inner, pane_scrollbars);
    if inner_rect == pane_inner {
        return (inner_rect, None);
    }
    let gutter = Rect::new(
        pane_inner.x + pane_inner.width.saturating_sub(1),
        pane_inner.y,
        1,
        pane_inner.height,
    );
    let scrollbar_rect = rt
        .scroll_metrics()
        .filter(|metrics| should_show_scrollbar(*metrics))
        .map(|_| gutter);

    (inner_rect, scrollbar_rect)
}

/// Resize every visible runtime in a tab to the geometry it would receive if the tab were selected.
pub(super) fn resize_tab_panes(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    workspace_index: usize,
    tab: &crate::workspace::Tab,
    area: Rect,
    cell_size: crate::kitty_graphics::HostCellSize,
) {
    let multi_pane = tab.layout.pane_count() > 1;

    if tab.zoomed {
        let focused_id = tab.layout.focused();
        if let Some((_, rt)) =
            runtime_for_tab_pane(app, terminal_runtimes, workspace_index, tab, focused_id)
        {
            let borders = if app.pane_borders.shows_borders(multi_pane) && app.pane_outer_borders {
                Borders::ALL
            } else {
                Borders::NONE
            };
            let pane_inner = pane_inner_rect(area, borders);
            let inner_rect = terminal_inner_rect(rt, pane_inner, app.pane_scrollbars);
            rt.resize(
                inner_rect.height,
                inner_rect.width,
                cell_size.width_px,
                cell_size.height_px,
            );
        }
        return;
    }

    for info in apply_pane_chrome(
        tab.layout.panes(area),
        app.pane_borders,
        app.pane_gaps,
        app.pane_outer_borders,
    ) {
        let pane_inner = pane_inner_rect(info.rect, info.borders);

        if let Some((_, rt)) =
            runtime_for_tab_pane(app, terminal_runtimes, workspace_index, tab, info.id)
        {
            let inner_rect = terminal_inner_rect(rt, pane_inner, app.pane_scrollbars);
            rt.resize(
                inner_rect.height,
                inner_rect.width,
                cell_size.width_px,
                cell_size.height_px,
            );
        }
    }
}

/// Compute pane layout info and optionally resize pane runtimes to match.
pub(super) fn compute_pane_infos_for_tab(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    ws_idx: usize,
    tab_idx: usize,
    area: Rect,
    resize_panes: bool,
    cell_size: crate::kitty_graphics::HostCellSize,
) -> Vec<PaneInfo> {
    let Some(tab) = app
        .workspaces
        .get(ws_idx)
        .and_then(|workspace| workspace.tabs.get(tab_idx))
    else {
        return Vec::new();
    };

    let multi_pane = tab.layout.pane_count() > 1;

    if tab.zoomed {
        let focused_id = tab.layout.focused();
        let borders = if app.pane_borders.shows_borders(multi_pane) && app.pane_outer_borders {
            Borders::ALL
        } else {
            Borders::NONE
        };
        let pane_inner = pane_inner_rect(area, borders);
        let mut inner_rect = pane_inner;
        let mut scrollbar_rect = None;
        if let Some(rt) = app.runtime_for_pane_in_workspace(terminal_runtimes, ws_idx, focused_id) {
            (inner_rect, scrollbar_rect) =
                stable_scrollbar_gutter(rt, pane_inner, app.pane_scrollbars);
            if resize_panes {
                rt.resize(
                    inner_rect.height,
                    inner_rect.width,
                    cell_size.width_px,
                    cell_size.height_px,
                );
            }
        }
        return vec![PaneInfo {
            id: focused_id,
            rect: area,
            inner_rect,
            scrollbar_rect,
            borders,
            is_focused: true,
        }];
    }

    let mut pane_infos = apply_pane_chrome(
        tab.layout.panes(area),
        app.pane_borders,
        app.pane_gaps,
        app.pane_outer_borders,
    );

    for info in &mut pane_infos {
        let pane_inner = pane_inner_rect(info.rect, info.borders);

        let mut inner_rect = pane_inner;
        let mut scrollbar_rect = None;
        if let Some(rt) = app.runtime_for_pane_in_workspace(terminal_runtimes, ws_idx, info.id) {
            (inner_rect, scrollbar_rect) =
                stable_scrollbar_gutter(rt, pane_inner, app.pane_scrollbars);
            if resize_panes {
                rt.resize(
                    inner_rect.height,
                    inner_rect.width,
                    cell_size.width_px,
                    cell_size.height_px,
                );
            }
        }

        info.inner_rect = inner_rect;
        info.scrollbar_rect = scrollbar_rect;
    }

    pane_infos
}

#[cfg(test)]
fn compute_pane_infos(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    area: Rect,
    resize_panes: bool,
    cell_size: crate::kitty_graphics::HostCellSize,
) -> Vec<PaneInfo> {
    let Some(workspace_index) = app.active else {
        return Vec::new();
    };
    let Some(tab_index) = app
        .workspaces
        .get(workspace_index)
        .map(crate::workspace::Workspace::active_tab_index)
    else {
        return Vec::new();
    };
    compute_pane_infos_for_tab(
        app,
        terminal_runtimes,
        workspace_index,
        tab_index,
        area,
        resize_panes,
        cell_size,
    )
}

pub(super) fn render_panes(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    frame: &mut Frame,
    target: Option<super::layout::TabSurfaceTarget>,
    pane_infos: &[PaneInfo],
    split_borders: &[crate::layout::SplitBorder],
) {
    let Some(ws_idx) = target.map(|target| target.workspace_index) else {
        return;
    };
    let Some(ws) = app.workspaces.get(ws_idx) else {
        return;
    };

    for info in pane_infos {
        if let Some(rt) = app.runtime_for_pane_in_workspace(terminal_runtimes, ws_idx, info.id) {
            let show_cursor = info.is_focused && !pane_is_scrolled_back(rt);
            rt.render(frame, info.inner_rect, show_cursor);
            render_pane_scrollbar(app, frame, info, rt);
        }
    }

    render_pane_borders(app, ws, pane_infos, split_borders, frame);
}

#[cfg(test)]
mod tests {
    use super::super::chrome::pane_border_title;
    use super::super::text::display_width;
    use super::*;
    use crate::app::state::Palette;
    use crate::config::PaneBordersConfig;
    use crate::layout::PaneId;
    use crate::selection::Selection;
    use crate::terminal::TerminalRuntime;
    use crate::terminal::TerminalState;
    use crate::utils::render::widgets::selection::{
        automatic_selection_bg, automatic_selection_style, render_selection_highlight,
    };
    use crate::workspace::Workspace;
    use ratatui::style::{Color, Modifier, Style};

    include!("chrome/tests/chrome_test.rs");
    include!("tests/geometry_test.rs");
    include!("../../../utils/render/widgets/tests/selection_test.rs");
}
