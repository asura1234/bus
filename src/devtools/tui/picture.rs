//! `screenshot`: the driver's screen cells drawn into a PNG with a bundled
//! monospace font, for questions of layout, overlap and colour. It draws what
//! the cells hold; inline images (Kitty graphics) are not cells and need
//! `screenshot --real`.
use super::screen::{Cell, Grid};
use std::collections::HashMap;
use std::path::Path;
use unicode_width::UnicodeWidthStr;

/// DejaVu Sans Mono (Bitstream Vera licence, `assets/DejaVuSansMono-LICENSE.txt`).
const FONT: &[u8] = include_bytes!("assets/DejaVuSansMono.ttf");
pub(super) const CELL_W: usize = 10;
pub(super) const CELL_H: usize = 20;
const FONT_PX: f32 = 16.0;
const DEFAULT_FG: [u8; 3] = [0xd8, 0xd8, 0xd8];
const DEFAULT_BG: [u8; 3] = [0x18, 0x18, 0x1e];

struct Canvas {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
}

impl Canvas {
    fn new(width: usize, height: usize) -> Self {
        let mut pixels = Vec::with_capacity(width * height * 3);
        for _ in 0..width * height {
            pixels.extend_from_slice(&DEFAULT_BG);
        }
        Self {
            width,
            height,
            pixels,
        }
    }

    fn fill(&mut self, x: usize, y: usize, w: usize, h: usize, color: [u8; 3]) {
        for py in y..(y + h).min(self.height) {
            for px in x..(x + w).min(self.width) {
                let at = (py * self.width + px) * 3;
                self.pixels[at..at + 3].copy_from_slice(&color);
            }
        }
    }

    /// Blends `color` into one pixel with coverage `alpha` (0-255).
    fn blend(&mut self, x: usize, y: usize, color: [u8; 3], alpha: u8) {
        if x >= self.width || y >= self.height || alpha == 0 {
            return;
        }
        let at = (y * self.width + x) * 3;
        let a = u16::from(alpha);
        for (pixel, over) in self.pixels[at..at + 3].iter_mut().zip(color) {
            let under = u16::from(*pixel);
            *pixel = ((u16::from(over) * a + under * (255 - a)) / 255) as u8;
        }
    }
}

/// Draws `grid` and writes it to `path`; returns the image size in pixels.
pub(super) fn write_png(grid: &Grid, path: &Path) -> Result<(usize, usize), String> {
    let font = fontdue::Font::from_bytes(FONT, fontdue::FontSettings::default())
        .map_err(|e| format!("bundled font: {e}"))?;
    let ascent = font
        .horizontal_line_metrics(FONT_PX)
        .map_or(FONT_PX * 0.8, |m| m.ascent);
    let baseline = ((CELL_H as f32 - FONT_PX) / 2.0 + ascent).round() as i32;
    let mut canvas = Canvas::new(
        usize::from(grid.cols) * CELL_W,
        usize::from(grid.rows) * CELL_H,
    );
    let mut glyphs: HashMap<char, (fontdue::Metrics, Vec<u8>)> = HashMap::new();
    // Backgrounds first: a glyph that descends below its cell (`_`, `g`) must
    // not be painted over by the next row's background.
    for (row, cells) in grid.cells.iter().enumerate() {
        for (col, cell) in cells.iter().enumerate() {
            canvas.fill(
                col * CELL_W,
                row * CELL_H,
                CELL_W,
                CELL_H,
                cell_colors(cell).1,
            );
        }
    }
    for (row, cells) in grid.cells.iter().enumerate() {
        let mut col = 0;
        while col < cells.len() {
            let cell = &cells[col];
            let span = cell.symbol.width().clamp(1, 2);
            let (fg, _) = cell_colors(cell);
            let (x, y) = (col * CELL_W, row * CELL_H);
            if let Some(ch) = cell.symbol.chars().next().filter(|c| !c.is_whitespace()) {
                if !draw_box(&mut canvas, ch, x, y, fg) {
                    let glyph = glyphs
                        .entry(ch)
                        .or_insert_with(|| font.rasterize(ch, FONT_PX));
                    if font.lookup_glyph_index(ch) == 0 {
                        outline(&mut canvas, x + 1, y + 3, CELL_W * span - 2, CELL_H - 6, fg);
                    } else {
                        draw_glyph(&mut canvas, glyph, x, y as i32 + baseline, fg, cell);
                    }
                }
            }
            if cell.attrs.contains(&"underline") || cell.link.is_some() {
                canvas.fill(x, y + CELL_H - 3, CELL_W * span, 1, fg);
            }
            if cell.attrs.contains(&"strike") {
                canvas.fill(x, y + CELL_H / 2, CELL_W * span, 1, fg);
            }
            col += span;
        }
    }
    encode(&canvas, path)?;
    Ok((canvas.width, canvas.height))
}

fn draw_glyph(
    canvas: &mut Canvas,
    (metrics, coverage): &(fontdue::Metrics, Vec<u8>),
    x: usize,
    baseline: i32,
    fg: [u8; 3],
    cell: &Cell,
) {
    let left = x as i32 + metrics.xmin;
    let top = baseline - metrics.height as i32 - metrics.ymin;
    // A second pass one pixel right stands in for a bold face, and a slant
    // about the baseline for an italic one.
    let passes = if cell.attrs.contains(&"bold") { 2 } else { 1 };
    let italic = cell.attrs.contains(&"italic");
    for pass in 0..passes {
        for gy in 0..metrics.height {
            let py = top + gy as i32;
            let slant = if italic {
                (baseline - py).div_euclid(4)
            } else {
                0
            };
            for gx in 0..metrics.width {
                let alpha = coverage[gy * metrics.width + gx];
                let px = left + gx as i32 + pass + slant;
                if px >= 0 && py >= 0 {
                    canvas.blend(px as usize, py as usize, fg, alpha);
                }
            }
        }
    }
}

fn outline(canvas: &mut Canvas, x: usize, y: usize, w: usize, h: usize, color: [u8; 3]) {
    canvas.fill(x, y, w, 1, color);
    canvas.fill(x, y + h - 1, w, 1, color);
    canvas.fill(x, y, 1, h, color);
    canvas.fill(x + w - 1, y, 1, h, color);
}

/// Box-drawing and block characters drawn as lines across the whole cell, so
/// borders join up the way a terminal draws them. Returns false for others.
fn draw_box(canvas: &mut Canvas, ch: char, x: usize, y: usize, fg: [u8; 3]) -> bool {
    let (cx, cy) = (x + CELL_W / 2, y + CELL_H / 2);
    match ch {
        '█' => canvas.fill(x, y, CELL_W, CELL_H, fg),
        '▀' => canvas.fill(x, y, CELL_W, CELL_H / 2, fg),
        '▄' => canvas.fill(x, cy, CELL_W, CELL_H - CELL_H / 2, fg),
        '▌' => canvas.fill(x, y, CELL_W / 2, CELL_H, fg),
        '▐' => canvas.fill(cx, y, CELL_W - CELL_W / 2, CELL_H, fg),
        _ => {
            let Some((up, down, left, right, thick)) = box_arms(ch) else {
                return false;
            };
            let t = if thick { 2 } else { 1 };
            if left {
                canvas.fill(x, cy, CELL_W / 2 + t, t, fg);
            }
            if right {
                canvas.fill(cx, cy, CELL_W - CELL_W / 2, t, fg);
            }
            if up {
                canvas.fill(cx, y, t, CELL_H / 2 + t, fg);
            }
            if down {
                canvas.fill(cx, cy, t, CELL_H - CELL_H / 2, fg);
            }
        }
    }
    true
}

/// Which arms of a box-drawing character are drawn: up, down, left, right, heavy.
fn box_arms(ch: char) -> Option<(bool, bool, bool, bool, bool)> {
    let arms = match ch {
        '─' | '╌' | '┄' => (false, false, true, true, false),
        '━' => (false, false, true, true, true),
        '│' | '╎' | '┆' => (true, true, false, false, false),
        '┃' => (true, true, false, false, true),
        '┌' | '╭' => (false, true, false, true, false),
        '┐' | '╮' => (false, true, true, false, false),
        '└' | '╰' => (true, false, false, true, false),
        '┘' | '╯' => (true, false, true, false, false),
        '┏' => (false, true, false, true, true),
        '┓' => (false, true, true, false, true),
        '┗' => (true, false, false, true, true),
        '┛' => (true, false, true, false, true),
        '├' => (true, true, false, true, false),
        '┤' => (true, true, true, false, false),
        '┬' => (false, true, true, true, false),
        '┴' => (true, false, true, true, false),
        '┼' => (true, true, true, true, false),
        '═' => (false, false, true, true, true),
        '║' => (true, true, false, false, true),
        _ => return None,
    };
    Some(arms)
}

fn cell_colors(cell: &Cell) -> ([u8; 3], [u8; 3]) {
    let fg = parse_color(&cell.fg).unwrap_or(DEFAULT_FG);
    let bg = parse_color(&cell.bg).unwrap_or(DEFAULT_BG);
    let (mut fg, bg) = if cell.attrs.contains(&"inverse") {
        (bg, fg)
    } else {
        (fg, bg)
    };
    if cell.attrs.contains(&"dim") {
        fg = fg.map(|c| ((u16::from(c) + u16::from(DEFAULT_BG[0])) / 2) as u8);
    }
    if cell.attrs.contains(&"hidden") {
        fg = bg;
    }
    (fg, bg)
}

/// `#rrggbb`, `idx:N` or a named ANSI colour, as the screen reports cells.
pub(super) fn parse_color(name: &str) -> Option<[u8; 3]> {
    if let Some(hex) = name.strip_prefix('#') {
        let value = u32::from_str_radix(hex, 16).ok()?;
        return Some([(value >> 16) as u8, (value >> 8) as u8, value as u8]);
    }
    if let Some(index) = name.strip_prefix("idx:") {
        return Some(xterm_color(index.parse().ok()?));
    }
    const NAMES: [&str; 16] = [
        "black",
        "red",
        "green",
        "yellow",
        "blue",
        "magenta",
        "cyan",
        "gray",
        "darkgray",
        "lightred",
        "lightgreen",
        "lightyellow",
        "lightblue",
        "lightmagenta",
        "lightcyan",
        "white",
    ];
    NAMES
        .iter()
        .position(|n| *n == name)
        .map(|index| xterm_color(index as u8))
}

/// The standard xterm 256-colour palette.
fn xterm_color(index: u8) -> [u8; 3] {
    const BASE: [[u8; 3]; 16] = [
        [0, 0, 0],
        [205, 49, 49],
        [13, 188, 121],
        [229, 229, 16],
        [36, 114, 200],
        [188, 63, 188],
        [17, 168, 205],
        [229, 229, 229],
        [102, 102, 102],
        [241, 76, 76],
        [35, 209, 139],
        [245, 245, 67],
        [59, 142, 234],
        [214, 112, 214],
        [41, 184, 219],
        [255, 255, 255],
    ];
    match index {
        0..=15 => BASE[usize::from(index)],
        16..=231 => {
            let i = index - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            [level(i / 36), level((i / 6) % 6), level(i % 6)]
        }
        _ => {
            let gray = 8 + (index - 232) * 10;
            [gray, gray, gray]
        }
    }
}

fn encode(canvas: &Canvas, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut encoder = png::Encoder::new(
        std::io::BufWriter::new(file),
        canvas.width as u32,
        canvas.height as u32,
    );
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer
        .write_image_data(&canvas.pixels)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
#[path = "tests/picture_test.rs"]
mod tests;
