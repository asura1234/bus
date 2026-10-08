use super::frame::{CellData, CursorState, FrameData};
use super::surface::SurfaceRect;
use std::collections::HashMap;

impl CellData {
    #[allow(deprecated)]
    pub(crate) fn from_ratatui_cell(cell: &ratatui::buffer::Cell) -> Self {
        Self {
            symbol: cell.symbol().to_owned(),
            fg: color_to_u32(cell.fg),
            bg: color_to_u32(cell.bg),
            modifier: modifier_to_u16(cell.modifier),
            skip: matches!(cell.diff_option, ratatui::buffer::CellDiffOption::Skip)
                || (cell.skip && matches!(cell.diff_option, ratatui::buffer::CellDiffOption::None)),
            hyperlink: None,
        }
    }
}

impl FrameData {
    /// Creates a `FrameData` from a ratatui `Buffer` and optional cursor.
    ///
    /// This converts ratatui's internal cell representation into the
    /// wire-protocol cell format. The conversion is lossless for all
    /// commonly used cell attributes.
    #[cfg(test)]
    pub fn from_ratatui_buffer(
        buffer: &ratatui::buffer::Buffer,
        cursor: Option<CursorState>,
    ) -> Self {
        Self::from_ratatui_buffer_with_hyperlinks(buffer, cursor, &[])
    }

    pub fn from_ratatui_buffer_with_hyperlinks(
        buffer: &ratatui::buffer::Buffer,
        cursor: Option<CursorState>,
        hyperlinks: &[((u16, u16), String, String)],
    ) -> Self {
        let area = buffer.area;
        let width = area.width;
        let height = area.height;

        let mut hyperlink_uris = Vec::<String>::new();
        let mut hyperlink_indices = HashMap::<&str, u32>::new();
        let mut hyperlink_by_position = HashMap::<(u16, u16), (&str, &str)>::new();
        for ((x, y), symbol, uri) in hyperlinks {
            hyperlink_by_position.insert((*x, *y), (symbol.as_str(), uri.as_str()));
        }
        let mut cells = Vec::with_capacity((width as usize) * (height as usize));
        for row in 0..height {
            for col in 0..width {
                let cell = buffer.cell((col, row)).expect("cell within bounds");
                let hyperlink = hyperlink_by_position
                    .get(&(col, row))
                    .and_then(|(symbol, uri)| {
                        if *symbol != cell.symbol() {
                            return None;
                        }
                        Some(*hyperlink_indices.entry(*uri).or_insert_with(|| {
                            let index = hyperlink_uris.len() as u32;
                            hyperlink_uris.push((*uri).to_owned());
                            index
                        }))
                    });
                let mut cell = CellData::from_ratatui_cell(cell);
                cell.hyperlink = hyperlink;
                cells.push(cell);
            }
        }

        FrameData {
            cells,
            width,
            height,
            cursor,
            hyperlinks: hyperlink_uris,
            graphics: Vec::new(),
        }
    }

    pub(crate) fn replace_from_ratatui_buffer_preserving_effects(
        &mut self,
        buffer: &ratatui::buffer::Buffer,
        cursor: Option<CursorState>,
    ) {
        let width = self.width;
        let hyperlinks = if width == 0 {
            Vec::new()
        } else {
            self.cells
                .iter()
                .enumerate()
                .filter_map(|(index, cell)| {
                    let uri = self.hyperlinks.get(cell.hyperlink? as usize)?;
                    let x = u16::try_from(index % usize::from(width)).ok()?;
                    let y = u16::try_from(index / usize::from(width)).ok()?;
                    Some(((x, y), cell.symbol.clone(), uri.clone()))
                })
                .collect::<Vec<_>>()
        };
        let graphics = std::mem::take(&mut self.graphics);
        let mut replacement =
            Self::from_ratatui_buffer_with_hyperlinks(buffer, cursor, &hyperlinks);
        replacement.graphics = graphics;
        *self = replacement;
    }

    /// Reconstructs a ratatui `Buffer` from this frame data.
    ///
    /// Returns `None` if the cells vector length doesn't match `width * height`.
    pub(crate) fn to_ratatui_buffer(&self) -> Option<ratatui::buffer::Buffer> {
        let expected = (self.width as usize) * (self.height as usize);
        if self.cells.len() != expected {
            return None;
        }

        let area = ratatui::layout::Rect::new(0, 0, self.width, self.height);
        let mut buffer = ratatui::buffer::Buffer::filled(area, ratatui::buffer::Cell::new(" "));

        for row in 0..self.height {
            for col in 0..self.width {
                let idx = (row as usize) * (self.width as usize) + (col as usize);
                let cell_data = &self.cells[idx];
                let cell = buffer.cell_mut((col, row)).expect("cell within bounds");
                cell.set_symbol(&cell_data.symbol);
                cell.fg = u32_to_color(cell_data.fg);
                cell.bg = u32_to_color(cell_data.bg);
                cell.modifier = u16_to_modifier(cell_data.modifier);
                cell.set_diff_option(if cell_data.skip {
                    ratatui::buffer::CellDiffOption::Skip
                } else {
                    ratatui::buffer::CellDiffOption::None
                });
            }
        }

        Some(buffer)
    }
}

/// Converts a ratatui `Color` to a packed u32 for wire transport.
///
/// Encoding:
/// - Named colors (Reset, Black, …, White) → `0x00_00_00_XX` where XX is 0..=16
/// - Indexed palette → `0x01_00_00_XX` where XX is the palette index
/// - RGB → `0x02_RR_GG_BB` with components in the lower 3 bytes
pub(crate) fn color_to_u32(color: ratatui::style::Color) -> u32 {
    match color {
        ratatui::style::Color::Reset => 0x00_00_00_00,
        ratatui::style::Color::Black => 0x00_00_00_01,
        ratatui::style::Color::Red => 0x00_00_00_02,
        ratatui::style::Color::Green => 0x00_00_00_03,
        ratatui::style::Color::Yellow => 0x00_00_00_04,
        ratatui::style::Color::Blue => 0x00_00_00_05,
        ratatui::style::Color::Magenta => 0x00_00_00_06,
        ratatui::style::Color::Cyan => 0x00_00_00_07,
        ratatui::style::Color::Gray => 0x00_00_00_08,
        ratatui::style::Color::DarkGray => 0x00_00_00_09,
        ratatui::style::Color::LightRed => 0x00_00_00_0A,
        ratatui::style::Color::LightGreen => 0x00_00_00_0B,
        ratatui::style::Color::LightYellow => 0x00_00_00_0C,
        ratatui::style::Color::LightBlue => 0x00_00_00_0D,
        ratatui::style::Color::LightMagenta => 0x00_00_00_0E,
        ratatui::style::Color::LightCyan => 0x00_00_00_0F,
        ratatui::style::Color::White => 0x00_00_00_10,
        ratatui::style::Color::Indexed(i) => 0x01_00_00_00 | (i as u32),
        ratatui::style::Color::Rgb(r, g, b) => {
            0x02_00_00_00 | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
        }
    }
}

/// Converts a packed u32 back to a ratatui `Color`.
pub(super) fn u32_to_color(val: u32) -> ratatui::style::Color {
    match val >> 24 {
        0x00 => match val & 0xFF {
            0x00 => ratatui::style::Color::Reset,
            0x01 => ratatui::style::Color::Black,
            0x02 => ratatui::style::Color::Red,
            0x03 => ratatui::style::Color::Green,
            0x04 => ratatui::style::Color::Yellow,
            0x05 => ratatui::style::Color::Blue,
            0x06 => ratatui::style::Color::Magenta,
            0x07 => ratatui::style::Color::Cyan,
            0x08 => ratatui::style::Color::Gray,
            0x09 => ratatui::style::Color::DarkGray,
            0x0A => ratatui::style::Color::LightRed,
            0x0B => ratatui::style::Color::LightGreen,
            0x0C => ratatui::style::Color::LightYellow,
            0x0D => ratatui::style::Color::LightBlue,
            0x0E => ratatui::style::Color::LightMagenta,
            0x0F => ratatui::style::Color::LightCyan,
            0x10 => ratatui::style::Color::White,
            _ => ratatui::style::Color::Reset, // unknown named → Reset
        },
        0x01 => ratatui::style::Color::Indexed((val & 0xFF) as u8),
        0x02 => {
            let r = ((val >> 16) & 0xFF) as u8;
            let g = ((val >> 8) & 0xFF) as u8;
            let b = (val & 0xFF) as u8;
            ratatui::style::Color::Rgb(r, g, b)
        }
        _ => ratatui::style::Color::Reset, // unknown tag → Reset
    }
}

pub(super) const UNDERLINE_STYLE_SHIFT: u16 = 12;
pub(super) const UNDERLINE_STYLE_MASK: u16 = 0xF000;

/// Converts a ratatui `Modifier` bitmask to a u16 for wire transport.
pub(crate) fn modifier_to_u16(modifier: ratatui::style::Modifier) -> u16 {
    modifier.bits()
}

pub(crate) fn underline_style_from_modifier(modifier: u16) -> u8 {
    ((modifier & UNDERLINE_STYLE_MASK) >> UNDERLINE_STYLE_SHIFT) as u8
}

pub(crate) fn modifier_with_underline_style(
    modifier: ratatui::style::Modifier,
    underline_style: u8,
) -> ratatui::style::Modifier {
    let bits = modifier.bits() | ((u16::from(underline_style) & 0x0F) << UNDERLINE_STYLE_SHIFT);
    ratatui::style::Modifier::from_bits_retain(bits)
}

/// Converts a u16 back to a ratatui `Modifier`.
pub(super) fn u16_to_modifier(val: u16) -> ratatui::style::Modifier {
    ratatui::style::Modifier::from_bits_truncate(val & !UNDERLINE_STYLE_MASK)
}

impl From<ratatui::layout::Rect> for SurfaceRect {
    fn from(rect: ratatui::layout::Rect) -> Self {
        Self {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        }
    }
}
