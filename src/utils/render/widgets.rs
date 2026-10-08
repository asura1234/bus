use ratatui::style::Color;

use crate::app::state::Palette;

pub(crate) fn panel_contrast_fg(palette: &Palette) -> Color {
    match palette.panel_bg {
        Color::Reset => palette.surface_dim,
        color => color,
    }
}
