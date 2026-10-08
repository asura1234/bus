use ratatui::layout::Rect;

mod chrome;
mod draw;
mod layout;
mod scrollbar;

use crate::utils::render::status_popups as status;
use crate::utils::text::width as text;

pub(crate) use self::chrome::{apply_pane_chrome, pane_inner_rect};
pub(crate) use self::draw::pane_is_scrolled_back;
pub(crate) use self::layout::{
    compute_tab_surface, compute_tab_surface_for, render_tab_surface, resize_tab_surface,
    tab_surface_cursor, tab_surface_hyperlinks, TabSurfaceLayout, TabSurfaceTarget, TabSurfaceView,
};
pub(crate) use self::scrollbar::{
    render_pane_scrollbar_buffer, scrollbar_offset_from_drag_row, scrollbar_offset_from_row,
    scrollbar_thumb_grab_offset,
};

pub(crate) use self::status::{render_config_diagnostic_buffer, render_copy_feedback_buffer};
pub(crate) use crate::server::workspaces::agent_view::agent_panel_entries_from;

use crate::app::AppState;
use crate::terminal::TerminalRuntimeRegistry;

pub fn compute_view_with_runtime_registry(
    app: &mut AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    area: Rect,
) {
    compute_view_internal(
        app,
        terminal_runtimes,
        area,
        true,
        crate::kitty_graphics::HostCellSize::default(),
    );
}

pub(crate) fn compute_view_without_resizing_panes(
    app: &mut AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    area: Rect,
) {
    compute_view_internal(
        app,
        terminal_runtimes,
        area,
        false,
        crate::kitty_graphics::HostCellSize::default(),
    );
}

fn compute_view_internal(
    app: &mut AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    area: Rect,
    resize_panes: bool,
    cell_size: crate::kitty_graphics::HostCellSize,
) {
    let TabSurfaceLayout { pane_infos, .. } =
        compute_tab_surface(app, terminal_runtimes, area, resize_panes, cell_size);

    if resize_panes {
        resize_background_tab_panes(app, terminal_runtimes, area, cell_size);
    }

    app.view = crate::app::ViewState {
        terminal_area: area,
        pane_infos,
    };
}

fn resize_background_tab_panes(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    area: Rect,
    cell_size: crate::kitty_graphics::HostCellSize,
) {
    for (workspace_index, workspace) in app.workspaces.iter().enumerate() {
        for tab_index in 0..workspace.tabs.len() {
            if app.active == Some(workspace_index) && tab_index == workspace.active_tab_index() {
                continue;
            }
            resize_tab_surface(
                app,
                terminal_runtimes,
                workspace_index,
                tab_index,
                area,
                cell_size,
            );
        }
    }
}
