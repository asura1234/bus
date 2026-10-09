use crate::server::app_state::AppState;
use crate::server::workspaces::layout::PaneInfo;
use crate::utils::render::widgets::render_scrollbar_buffer;
use ratatui::{buffer::Buffer, layout::Rect, Frame};

pub(crate) fn pane_scrollbar_rect(info: &PaneInfo) -> Option<Rect> {
    info.scrollbar_rect
}

pub(crate) fn should_show_scrollbar(metrics: crate::utils::render::widgets::ScrollMetrics) -> bool {
    metrics.max_offset_from_bottom > 0
}

pub(crate) fn render_pane_scrollbar_buffer(
    buffer: &mut Buffer,
    metrics: crate::utils::render::widgets::ScrollMetrics,
    track: Rect,
    palette: &crate::utils::theme::Palette,
    focused: bool,
) {
    let (track_color, thumb_color, thumb_symbol) = if focused {
        (palette.overlay0, palette.overlay1, "▐")
    } else {
        (palette.surface_dim, palette.overlay0, "▕")
    };
    render_scrollbar_buffer(
        buffer,
        metrics,
        track,
        track_color,
        thumb_color,
        thumb_symbol,
    );
}

pub(super) fn render_pane_scrollbar(
    app: &AppState,
    frame: &mut Frame,
    info: &PaneInfo,
    rt: &crate::terminal::TerminalRuntime,
) {
    let Some(metrics) = rt.scroll_metrics() else {
        return;
    };
    let Some(track) = pane_scrollbar_rect(info) else {
        return;
    };
    render_pane_scrollbar_buffer(
        frame.buffer_mut(),
        metrics,
        track,
        &app.palette,
        info.is_focused,
    );
}
