//! Inline image thumbnails for room-history attachments, drawn with the Kitty
//! graphics protocol. History reserves cell rows for each thumbnail; this
//! module sizes them, decodes each image once, and emits only the placement
//! changes between frames. Without Kitty support, a known cell size, or a
//! readable image, history keeps showing the attachment's file name only.
use crate::kitty_graphics::{encode_delete_placement, encode_kitty_data, HostCellSize};
use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Tallest thumbnail, in cell rows.
pub(super) const MAX_ROWS: u16 = 8;
/// Widest thumbnail, in cell columns.
pub(super) const MAX_COLS: u16 = 48;
/// Larger files are never decoded on the UI thread.
const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
/// Kitty image ids for thumbnails start here, clear of pane image ids that
/// set the high bit and of small ids applications pick for themselves.
const FIRST_IMAGE_ID: u32 = 0x4255_0000;

/// One thumbnail shown in the history viewport.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Placement {
    pub path: Arc<Path>,
    pub x: u16,
    pub y: u16,
    pub cols: u16,
    pub rows: u16,
}

impl Placement {
    pub fn rect(&self) -> ratatui::layout::Rect {
        ratatui::layout::Rect::new(self.x, self.y, self.cols, self.rows)
    }
}

#[derive(Default)]
pub(super) struct Thumbnails {
    /// Host cell size when the terminal can show Kitty graphics.
    cell: Option<HostCellSize>,
    dims: HashMap<PathBuf, Option<(u32, u32)>>,
    /// Image id per decoded size, or None when the file cannot be decoded.
    images: HashMap<(PathBuf, u16, u16), Option<u32>>,
    /// Decoded thumbnails not yet sent to the terminal.
    pending_upload: HashMap<u32, Vec<u8>>,
    uploaded: HashSet<u32>,
    /// Placements drawn by the last encoded frame: image id, placement id.
    shown: Vec<(u32, u32, Placement)>,
    next_id: u32,
}

impl Thumbnails {
    /// `cell` is None when the host cannot show Kitty graphics.
    pub fn set_cell(&mut self, cell: Option<HostCellSize>) {
        let cell = cell.filter(|cell| cell.width_px > 1 && cell.height_px > 1);
        if cell != self.cell {
            // Thumbnails are rendered for one cell size; resize them.
            self.cell = cell;
            self.images.clear();
            self.pending_upload.clear();
        }
    }

    /// Identifies the inputs to thumbnail layout, for history caching.
    pub fn layout_key(&self) -> Option<(u32, u32)> {
        self.cell.map(|cell| (cell.width_px, cell.height_px))
    }

    /// Cell size of a readable image attachment, or None to show only its name.
    pub fn size(&mut self, path: &Path, max_cols: u16) -> Option<(u16, u16)> {
        let cell = self.cell?;
        if !is_image(path) {
            return None;
        }
        let (width, height) = *self
            .dims
            .entry(path.to_owned())
            .or_insert_with(|| dimensions(path))
            .as_ref()?;
        Some(fit((width, height), cell, max_cols.min(MAX_COLS), MAX_ROWS))
    }

    /// Kitty commands that move the visible thumbnails from the previous
    /// frame's placements to `placements`.
    pub fn encode(&mut self, placements: &[Placement]) -> Vec<u8> {
        let Some(cell) = self.cell else {
            return self.clear();
        };
        let mut next = Vec::with_capacity(placements.len());
        let mut copies: HashMap<u32, u32> = HashMap::new();
        for placement in placements {
            let key = (placement.path.to_path_buf(), placement.cols, placement.rows);
            let id = match self.images.get(&key) {
                Some(id) => *id,
                None => {
                    let id = self.next_image_id();
                    let data = thumbnail_png(&placement.path, cell, placement.cols, placement.rows);
                    let id = data.map(|data| {
                        self.pending_upload.insert(id, data);
                        id
                    });
                    self.images.insert(key, id);
                    id
                }
            };
            let Some(id) = id else { continue };
            // One image shown twice needs a placement id per copy.
            let copy = copies.entry(id).or_insert(0);
            *copy += 1;
            next.push((id, *copy, placement.clone()));
        }
        if next == self.shown {
            return Vec::new();
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"\x1b7");
        for (id, copy, old) in &self.shown {
            if !next.contains(&(*id, *copy, old.clone())) {
                encode_delete_placement(&mut out, *id, *copy);
            }
        }
        for (id, copy, placement) in &next {
            if self.shown.contains(&(*id, *copy, placement.clone())) {
                continue;
            }
            if let Some(data) = self.pending_upload.remove(id) {
                encode_kitty_data(&mut out, &format!("a=t,t=d,f=100,i={id},q=2"), &data);
                self.uploaded.insert(*id);
            }
            if !self.uploaded.contains(id) {
                continue;
            }
            let _ = write!(
                out,
                "\x1b[{};{}H\x1b_Ga=p,i={id},p={copy},c={},r={},C=1,q=2;\x1b\\",
                placement.y + 1,
                placement.x + 1,
                placement.cols,
                placement.rows,
            );
        }
        out.extend_from_slice(b"\x1b8");
        self.shown = next;
        out
    }

    fn clear(&mut self) -> Vec<u8> {
        if self.shown.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (id, copy, _) in self.shown.drain(..) {
            encode_delete_placement(&mut out, id, copy);
        }
        out
    }

    fn next_image_id(&mut self) -> u32 {
        let id = FIRST_IMAGE_ID + self.next_id;
        self.next_id = self.next_id.wrapping_add(1) % 0x1_0000;
        id
    }
}

/// Recognizes the attachment types Bus decodes into thumbnails.
pub(super) fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp"
            )
        })
}

/// Fits an image of `pixels` into at most `max_cols` × `max_rows` cells,
/// preserving its aspect ratio and never enlarging it past its own size.
pub(super) fn fit(
    (width, height): (u32, u32),
    cell: HostCellSize,
    max_cols: u16,
    max_rows: u16,
) -> (u16, u16) {
    let (width, height) = (f64::from(width.max(1)), f64::from(height.max(1)));
    let (cell_w, cell_h) = (f64::from(cell.width_px), f64::from(cell.height_px));
    let scale = (f64::from(max_cols.max(1)) * cell_w / width)
        .min(f64::from(max_rows.max(1)) * cell_h / height)
        .min(1.0);
    let cols = ((width * scale / cell_w).round() as u16).clamp(1, max_cols.max(1));
    let rows = ((height * scale / cell_h).round() as u16).clamp(1, max_rows.max(1));
    (cols, rows)
}

fn dimensions(path: &Path) -> Option<(u32, u32)> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return None;
    }
    image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

/// Decodes `path` (the first frame of an animation) and re-encodes it as a PNG
/// no larger than the cells it covers.
fn thumbnail_png(path: &Path, cell: HostCellSize, cols: u16, rows: u16) -> Option<Vec<u8>> {
    let image = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .map_err(|error| tracing::warn!(event = "bus.thumbnail.decode_failed", %error))
        .ok()?;
    let image = image.thumbnail(
        u32::from(cols) * cell.width_px,
        u32::from(rows) * cell.height_px,
    );
    let mut png = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some(png)
}

/// Whether the host terminal is one known to draw Kitty graphics. Bus clients
/// run directly in the host terminal, so its environment identifies it.
pub(super) fn host_supports_kitty_graphics(var: impl Fn(&str) -> Option<String>) -> bool {
    if var("TMUX").is_some() {
        return false;
    }
    let term = var("TERM").unwrap_or_default();
    let program = var("TERM_PROGRAM").unwrap_or_default();
    term.contains("kitty")
        || term.contains("ghostty")
        || matches!(program.as_str(), "ghostty" | "WezTerm" | "kitty")
        || var("KITTY_WINDOW_ID").is_some()
        || var("GHOSTTY_RESOURCES_DIR").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELL: HostCellSize = HostCellSize {
        width_px: 10,
        height_px: 20,
    };

    fn write_png(dir: &Path, name: &str, size: (u32, u32)) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        image::RgbaImage::from_pixel(size.0, size.1, image::Rgba([200, 40, 40, 255]))
            .save(&path)
            .unwrap();
        path
    }

    fn scratch(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "bus-thumbnails-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn fit_bounds_rows_and_columns_and_keeps_the_aspect_ratio() {
        // A wide screenshot is limited by columns.
        assert_eq!(fit((4000, 1000), CELL, 48, 8), (48, 6));
        // A tall image is limited by rows.
        assert_eq!(fit((400, 4000), CELL, 48, 8), (2, 8));
        // Small images are never enlarged.
        assert_eq!(fit((30, 40), CELL, 48, 8), (3, 2));
        // Narrow history columns shrink the thumbnail too.
        assert_eq!(fit((4000, 1000), CELL, 20, 8), (20, 3));
        assert_eq!(fit((1, 1), CELL, 48, 8), (1, 1));
    }

    #[test]
    fn only_readable_images_on_a_kitty_host_get_a_size() {
        let dir = scratch("size");
        let png = write_png(&dir, "shot.png", (400, 200));
        let text = dir.join("notes.png.txt");
        std::fs::write(&text, "not an image").unwrap();
        let fake = dir.join("fake.png");
        std::fs::write(&fake, "not a png").unwrap();
        let mut thumbnails = Thumbnails::default();

        assert_eq!(thumbnails.size(&png, 80), None, "no Kitty host, no size");
        thumbnails.set_cell(Some(HostCellSize {
            width_px: 1,
            height_px: 1,
        }));
        assert_eq!(thumbnails.size(&png, 80), None, "unknown cell size");
        thumbnails.set_cell(Some(CELL));
        assert_eq!(thumbnails.size(&png, 80), Some((32, 8)));
        assert_eq!(thumbnails.size(&text, 80), None);
        assert_eq!(thumbnails.size(&fake, 80), None);
        assert_eq!(thumbnails.size(&dir.join("missing.png"), 80), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn image_types_are_recognized_case_insensitively() {
        for name in ["a.png", "a.JPG", "a.jpeg", "a.gif", "a.webp"] {
            assert!(is_image(Path::new(name)), "{name}");
        }
        for name in ["a.svg", "a.md", "png"] {
            assert!(!is_image(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn encode_uploads_once_and_sends_only_placement_changes() {
        let dir = scratch("encode");
        let path: Arc<Path> = Arc::from(write_png(&dir, "shot.png", (40, 40)).as_path());
        let mut thumbnails = Thumbnails::default();
        thumbnails.set_cell(Some(CELL));
        let at = |y| Placement {
            path: Arc::clone(&path),
            x: 3,
            y,
            cols: 4,
            rows: 2,
        };

        let first = String::from_utf8(thumbnails.encode(&[at(5)])).unwrap();
        assert!(first.contains("a=t,t=d,f=100,i=1112866816,q=2"), "{first}");
        assert!(first.contains("\x1b[6;4H\x1b_Ga=p,i=1112866816,p=1,c=4,r=2,C=1,q=2;"));
        assert!(first.starts_with("\x1b7") && first.ends_with("\x1b8"));
        assert!(thumbnails.encode(&[at(5)]).is_empty(), "unchanged frame");

        let moved = String::from_utf8(thumbnails.encode(&[at(7)])).unwrap();
        assert!(!moved.contains("a=t"), "data is uploaded once: {moved}");
        assert!(moved.contains("a=d,d=i,i=1112866816,p=1"));
        assert!(moved.contains("\x1b[8;4H"));

        let twice = String::from_utf8(thumbnails.encode(&[at(1), at(7)])).unwrap();
        assert!(twice.contains("p=2"), "a second copy has its own placement");

        let hidden = String::from_utf8(thumbnails.encode(&[])).unwrap();
        assert!(hidden.contains("p=1") && hidden.contains("p=2"));
        assert!(!hidden.contains("a=p"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn losing_kitty_support_deletes_shown_placements() {
        let dir = scratch("disable");
        let path: Arc<Path> = Arc::from(write_png(&dir, "shot.gif", (40, 40)).as_path());
        let mut thumbnails = Thumbnails::default();
        thumbnails.set_cell(Some(CELL));
        let placement = Placement {
            path,
            x: 0,
            y: 0,
            cols: 4,
            rows: 2,
        };
        assert!(!thumbnails
            .encode(std::slice::from_ref(&placement))
            .is_empty());
        thumbnails.set_cell(None);
        let cleared = String::from_utf8(thumbnails.encode(&[placement])).unwrap();
        assert!(cleared.contains("a=d,d=i"));
        assert!(!cleared.contains("a=p"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn host_detection_trusts_known_kitty_terminals_outside_tmux() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |key: &str| {
                pairs
                    .iter()
                    .find(|(name, _)| *name == key)
                    .map(|(_, value)| value.to_string())
            }
        };
        assert!(host_supports_kitty_graphics(env(&[(
            "TERM",
            "xterm-kitty"
        )])));
        assert!(host_supports_kitty_graphics(env(&[(
            "TERM_PROGRAM",
            "ghostty"
        )])));
        assert!(host_supports_kitty_graphics(env(&[(
            "TERM_PROGRAM",
            "WezTerm"
        )])));
        assert!(!host_supports_kitty_graphics(env(&[(
            "TERM_PROGRAM",
            "Apple_Terminal"
        )])));
        assert!(!host_supports_kitty_graphics(env(&[
            ("TERM", "xterm-kitty"),
            ("TMUX", "/tmp/tmux")
        ])));
    }
}
