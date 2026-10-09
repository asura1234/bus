//! Inline image thumbnails for room-history attachments, drawn with the Kitty
//! graphics protocol (kitty, Ghostty, WezTerm, iTerm2 3.7+) or, in older
//! iTerm2, its inline image protocol. History reserves cell
//! rows for each thumbnail; this module sizes them, decodes each image once,
//! and emits only what changed between frames. Without a graphics protocol, a
//! known cell size, or a readable image, history keeps showing the
//! attachment's file name only (it always shows that row).
use crate::protocol::kitty::apc::encode_delete_placement;
use crate::protocol::kitty::HostCellSize;
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

/// How the host terminal draws images.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Protocol {
    /// Kitty graphics: placements float above the cells and are moved or
    /// deleted by id.
    #[default]
    Kitty,
    /// iTerm2 before 3.7, `OSC 1337 File=`: the image is painted into the
    /// cells it covers, so it is redrawn after any repaint and erased by
    /// repainting its cells; every scroll step re-sends it.
    Iterm2,
}

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
    protocol: Protocol,
    /// Host cell size when the terminal can show images.
    cell: Option<HostCellSize>,
    dims: HashMap<PathBuf, Option<(u32, u32)>>,
    /// Image id per decoded size, or None when the file cannot be decoded.
    images: HashMap<(PathBuf, u16, u16), Option<u32>>,
    /// Decoded thumbnail PNGs per image id, kept so an image can be sent again
    /// after the terminal lost it without decoding the file again.
    decoded: HashMap<u32, Vec<u8>>,
    /// Image ids the terminal holds, as far as delivered frames tell.
    uploaded: HashSet<u32>,
    /// Placements drawn by the last encoded frame: image id, placement id.
    shown: Vec<(u32, u32, Placement)>,
    next_id: u32,
    /// iTerm2: the full inline image command per image id (each id has one
    /// cell size), resent whenever the image is drawn again.
    inline_data: HashMap<u32, String>,
    /// iTerm2: the screen was repainted under the shown images, erasing them.
    stale: bool,
    /// iTerm2: shown images moved or went away; their cells need repainting.
    repaint: bool,
    /// Kitty placements that may still be on screen after the terminal state
    /// was forgotten; deleted unless the next frame places them again.
    forgotten: Vec<(u32, u32)>,
}

impl Thumbnails {
    pub fn set_protocol(&mut self, protocol: Protocol) {
        if protocol != self.protocol {
            self.protocol = protocol;
            self.images.clear();
            self.decoded.clear();
            self.inline_data.clear();
        }
    }

    /// iTerm2 images live in the cells, so a full repaint erased them; draw
    /// them again with the next frame.
    pub fn invalidate(&mut self) {
        if self.protocol == Protocol::Iterm2 && !self.shown.is_empty() {
            self.stale = true;
        }
    }

    /// The terminal may not hold what earlier frames sent: a composed frame
    /// was never written (presentation was frozen), or the screen was cleared.
    /// Upload and place every visible image again with the next frame, and
    /// delete placements that may linger.
    pub fn forget_terminal(&mut self) {
        if self.uploaded.is_empty() && self.shown.is_empty() {
            return;
        }
        self.uploaded.clear();
        if self.protocol == Protocol::Iterm2 && !self.shown.is_empty() {
            // Inline images live in cells; repaint them away before redrawing.
            self.repaint = true;
        }
        self.forgotten
            .extend(self.shown.drain(..).map(|(id, copy, _)| (id, copy)));
        self.stale = true;
    }

    /// Whether erased images wait to be drawn again.
    pub fn stale(&self) -> bool {
        self.stale
    }

    /// Whether the last encode moved or removed iTerm2 images, so the client
    /// must repaint every cell to erase them before they are drawn again.
    pub fn take_repaint(&mut self) -> bool {
        std::mem::take(&mut self.repaint)
    }

    /// `cell` is None when the host cannot show images.
    pub fn set_cell(&mut self, cell: Option<HostCellSize>) {
        let cell = cell.filter(|cell| cell.width_px > 1 && cell.height_px > 1);
        if cell != self.cell {
            // Thumbnails are rendered for one cell size; resize them.
            self.cell = cell;
            self.images.clear();
            self.decoded.clear();
            self.inline_data.clear();
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

    /// Graphics commands that move the visible thumbnails from the previous
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
                        self.decoded.insert(id, data);
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
        if self.protocol == Protocol::Iterm2 {
            self.forgotten.clear();
            return self.encode_inline(next);
        }
        self.stale = false;
        if next == self.shown && self.forgotten.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"\x1b7");
        for (id, copy) in std::mem::take(&mut self.forgotten) {
            if !next
                .iter()
                .any(|(next_id, next_copy, _)| (*next_id, *next_copy) == (id, copy))
            {
                encode_delete_placement(&mut out, id, placement_id(id, copy));
            }
        }
        // A moved placement is re-placed under the same ids, which Kitty
        // graphics define as a flicker-free move; only placements that are
        // gone are deleted.
        for (id, copy, _) in &self.shown {
            if !next
                .iter()
                .any(|(next_id, next_copy, _)| (next_id, next_copy) == (id, copy))
            {
                encode_delete_placement(&mut out, *id, placement_id(*id, *copy));
            }
        }
        for (id, copy, placement) in &next {
            if self.shown.contains(&(*id, *copy, placement.clone())) {
                continue;
            }
            if !self.uploaded.contains(id) {
                if let Some(data) = self.decoded.get(id) {
                    encode_quiet_upload(&mut out, *id, data);
                    self.uploaded.insert(*id);
                }
            }
            if !self.uploaded.contains(id) {
                continue;
            }
            let _ = write!(
                out,
                "\x1b[{};{}H\x1b_Ga=p,i={id},p={},c={},r={},C=1,q=2;\x1b\\",
                placement.y + 1,
                placement.x + 1,
                placement_id(*id, *copy),
                placement.cols,
                placement.rows,
            );
        }
        out.extend_from_slice(b"\x1b8");
        self.shown = next;
        out
    }

    /// iTerm2 draws every shown image again whenever any of them changed or
    /// a repaint erased them; moved or removed ones are erased by a repaint.
    fn encode_inline(&mut self, next: Vec<(u32, u32, Placement)>) -> Vec<u8> {
        if next == self.shown && !self.stale {
            return Vec::new();
        }
        if next != self.shown && !self.shown.is_empty() {
            self.repaint = true;
        }
        self.stale = false;
        let mut out = Vec::new();
        if next.is_empty() {
            self.shown = next;
            return out;
        }
        out.extend_from_slice(b"\x1b7");
        for (id, _, placement) in &next {
            if !self.inline_data.contains_key(id) {
                if let Some(data) = self.decoded.get(id) {
                    self.inline_data.insert(
                        *id,
                        encode_inline_image(data, placement.cols, placement.rows),
                    );
                }
            }
            if let Some(command) = self.inline_data.get(id) {
                let _ = write!(
                    out,
                    "\x1b[{};{}H{command}",
                    placement.y + 1,
                    placement.x + 1,
                );
            }
        }
        out.extend_from_slice(b"\x1b8");
        self.shown = next;
        out
    }

    fn clear(&mut self) -> Vec<u8> {
        self.stale = false;
        let forgotten = std::mem::take(&mut self.forgotten);
        if self.shown.is_empty() && (forgotten.is_empty() || self.protocol == Protocol::Iterm2) {
            return Vec::new();
        }
        if self.protocol == Protocol::Iterm2 {
            self.shown.clear();
            self.stale = false;
            self.repaint = true;
            return Vec::new();
        }
        let mut out = Vec::new();
        for (id, copy) in forgotten
            .into_iter()
            .chain(self.shown.drain(..).map(|(id, copy, _)| (id, copy)))
        {
            encode_delete_placement(&mut out, id, placement_id(id, copy));
        }
        out
    }

    fn next_image_id(&mut self) -> u32 {
        let id = FIRST_IMAGE_ID + self.next_id;
        self.next_id = self.next_id.wrapping_add(1) % 0x1_0000;
        id
    }
}

/// The Kitty placement id of copy `copy` (from 1) of thumbnail image `id`,
/// unique across all thumbnails and stable while the copy stays on screen.
/// Kitty graphics name a placement by image id and placement id together, but
/// iTerm2 3.7 lets a placement replace another image's placement that has the
/// same placement id: two thumbnails in one message, both placed with `p=1`,
/// left only the last one drawn and a blank gap where the first belonged.
fn placement_id(id: u32, copy: u32) -> u32 {
    (id.wrapping_sub(FIRST_IMAGE_ID) << 8) | copy.min(0xff)
}

/// Kitty upload of `png` as image `id`, in chunks that each carry `q=2`:
/// iTerm2 answers a continuation chunk without it, and that reply would reach
/// the client's input.
pub(super) fn encode_quiet_upload(out: &mut Vec<u8>, id: u32, png: &[u8]) {
    use base64::Engine as _;
    // A multiple of 3 bytes, so each chunk's base64 has no padding mid-stream.
    const CHUNK_BYTES: usize = 3072;
    let mut chunks = png.chunks(CHUNK_BYTES).peekable();
    let mut first = true;
    while let Some(chunk) = chunks.next() {
        let more = u8::from(chunks.peek().is_some());
        let data = base64::engine::general_purpose::STANDARD.encode(chunk);
        if first {
            let _ = write!(out, "\x1b_Ga=t,t=d,f=100,i={id},q=2,m={more};{data}\x1b\\");
            first = false;
        } else {
            let _ = write!(out, "\x1b_Gm={more},q=2;{data}\x1b\\");
        }
    }
}

/// iTerm2's inline image command for `png`, scaled into `cols` × `rows` cells
/// without moving the cursor (https://iterm2.com/documentation-images.html).
pub(super) fn encode_inline_image(png: &[u8], cols: u16, rows: u16) -> String {
    use base64::Engine as _;
    format!(
        "\x1b]1337;File=inline=1;size={};width={cols};height={rows};preserveAspectRatio=1;doNotMoveCursor=1:{}\x07",
        png.len(),
        base64::engine::general_purpose::STANDARD.encode(png),
    )
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

/// The image protocol of the host terminal, or None when it draws no images
/// (Terminal.app, unknown terminals, anything inside tmux). Bus clients run
/// directly in the host terminal, so its environment identifies it.
pub(crate) fn host_graphics_protocol(var: impl Fn(&str) -> Option<String>) -> Option<Protocol> {
    if var("TMUX").is_some() {
        return None;
    }
    let term = var("TERM").unwrap_or_default();
    let program = var("TERM_PROGRAM").unwrap_or_default();
    if term.contains("kitty")
        || term.contains("ghostty")
        || matches!(program.as_str(), "ghostty" | "WezTerm" | "kitty")
        || var("KITTY_WINDOW_ID").is_some()
        || var("GHOSTTY_RESOURCES_DIR").is_some()
    {
        return Some(Protocol::Kitty);
    }
    // iTerm2 keeps TERM=xterm-256color; LC_TERMINAL also survives ssh.
    let version = if program == "iTerm.app" {
        var("TERM_PROGRAM_VERSION")
    } else if var("LC_TERMINAL").as_deref() == Some("iTerm2") {
        var("LC_TERMINAL_VERSION")
    } else {
        return None;
    };
    // iTerm2 3.7 draws Kitty placements sized in cells and moves them by id,
    // so scrolling never re-sends the image; older versions get inline images.
    let kitty = version.is_some_and(|version| {
        let mut parts = version
            .split('.')
            .map(|part| part.parse::<u32>().unwrap_or(0));
        (parts.next().unwrap_or(0), parts.next().unwrap_or(0)) >= (3, 7)
    });
    Some(if kitty {
        Protocol::Kitty
    } else {
        Protocol::Iterm2
    })
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

        // Scrolling moves it: the same ids are placed again, which moves the
        // placement without deleting it first (no blank frame).
        let moved = String::from_utf8(thumbnails.encode(&[at(7)])).unwrap();
        assert!(!moved.contains("a=t"), "data is uploaded once: {moved}");
        assert!(!moved.contains("a=d"), "a move never deletes: {moved}");
        assert!(moved.contains("\x1b[8;4H\x1b_Ga=p,i=1112866816,p=1,"));

        let twice = String::from_utf8(thumbnails.encode(&[at(1), at(7)])).unwrap();
        assert!(twice.contains("p=2"), "a second copy has its own placement");

        let hidden = String::from_utf8(thumbnails.encode(&[])).unwrap();
        assert!(hidden.contains("p=1") && hidden.contains("p=2"));
        assert!(!hidden.contains("a=p"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn forgetting_the_terminal_resends_visible_images_and_deletes_the_rest() {
        let dir = scratch("forget");
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
        assert!(!thumbnails.encode(&[at(5)]).is_empty());

        // Still visible: uploaded and placed again under the same ids.
        thumbnails.forget_terminal();
        let again = String::from_utf8(thumbnails.encode(&[at(5)])).unwrap();
        assert!(again.contains("a=t,t=d,f=100,i=1112866816,"), "{again}");
        assert!(again.contains("\x1b[6;4H\x1b_Ga=p,i=1112866816,p=1,"));
        assert!(!again.contains("a=d"), "{again}");

        // Gone from view: the lingering placement is deleted.
        thumbnails.forget_terminal();
        let gone = String::from_utf8(thumbnails.encode(&[])).unwrap();
        assert!(gone.contains("a=d,d=i,i=1112866816,p=1"), "{gone}");
        assert!(thumbnails.encode(&[]).is_empty());
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

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key: &str| {
            pairs
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| value.to_string())
        }
    }

    #[test]
    fn host_detection_picks_each_terminals_image_protocol() {
        use Protocol::*;
        for (pairs, expected) in [
            (&[("TERM", "xterm-kitty")][..], Some(Kitty)),
            (&[("TERM_PROGRAM", "ghostty")][..], Some(Kitty)),
            (&[("TERM_PROGRAM", "WezTerm")][..], Some(Kitty)),
            (&[("KITTY_WINDOW_ID", "1")][..], Some(Kitty)),
            // iTerm2 keeps TERM=xterm-256color; 3.7 and later draw Kitty
            // graphics, older versions only inline images.
            (
                &[
                    ("TERM_PROGRAM", "iTerm.app"),
                    ("TERM_PROGRAM_VERSION", "3.7.3"),
                    ("TERM", "xterm-256color"),
                ][..],
                Some(Kitty),
            ),
            (
                &[
                    ("TERM_PROGRAM", "iTerm.app"),
                    ("TERM_PROGRAM_VERSION", "3.10.0"),
                ][..],
                Some(Kitty),
            ),
            (
                &[
                    ("TERM_PROGRAM", "iTerm.app"),
                    ("TERM_PROGRAM_VERSION", "3.6.11"),
                ][..],
                Some(Iterm2),
            ),
            (&[("TERM_PROGRAM", "iTerm.app")][..], Some(Iterm2)),
            (
                &[("LC_TERMINAL", "iTerm2"), ("LC_TERMINAL_VERSION", "3.7.0")][..],
                Some(Kitty),
            ),
            (&[("LC_TERMINAL", "iTerm2")][..], Some(Iterm2)),
            (&[("TERM_PROGRAM", "Apple_Terminal")][..], None),
            (&[("TERM", "xterm-256color")][..], None),
            (&[("TERM", "xterm-kitty"), ("TMUX", "/tmp/tmux")][..], None),
            (
                &[("TERM_PROGRAM", "iTerm.app"), ("TMUX", "/tmp/tmux")][..],
                None,
            ),
        ] {
            assert_eq!(host_graphics_protocol(env(pairs)), expected, "{pairs:?}");
        }
    }

    #[test]
    fn kitty_uploads_keep_every_chunk_quiet() {
        use base64::Engine as _;
        let png: Vec<u8> = (0..7000u32).map(|n| n as u8).collect();
        let mut out = Vec::new();
        encode_quiet_upload(&mut out, 9, &png);
        let out = String::from_utf8(out).unwrap();
        let chunks: Vec<_> = out
            .split("\x1b\\")
            .filter(|chunk| !chunk.is_empty())
            .collect();
        assert_eq!(chunks.len(), 3, "7000 bytes in 3072-byte chunks");
        assert!(chunks[0].starts_with("\x1b_Ga=t,t=d,f=100,i=9,q=2,m=1;"));
        assert!(chunks[1].starts_with("\x1b_Gm=1,q=2;"));
        assert!(chunks[2].starts_with("\x1b_Gm=0,q=2;"));
        let decoded: Vec<u8> = chunks
            .iter()
            .flat_map(|chunk| {
                base64::engine::general_purpose::STANDARD
                    .decode(chunk.split_once(';').unwrap().1)
                    .unwrap()
            })
            .collect();
        assert_eq!(decoded, png);
    }

    #[test]
    fn iterm2_inline_image_follows_the_documented_format() {
        use base64::Engine as _;
        let png = b"\x89PNG fake bytes";
        let command = encode_inline_image(png, 12, 3);
        let prefix = format!(
            "\x1b]1337;File=inline=1;size={};width=12;height=3;preserveAspectRatio=1;doNotMoveCursor=1:",
            png.len()
        );
        assert!(command.starts_with(&prefix), "{command:?}");
        assert!(command.ends_with('\x07'), "BEL terminates the OSC");
        let payload = &command[prefix.len()..command.len() - 1];
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(payload)
                .unwrap(),
            png
        );
    }

    #[test]
    fn iterm2_redraws_images_after_repaints_and_erases_moved_ones() {
        let dir = scratch("iterm2");
        let path: Arc<Path> = Arc::from(write_png(&dir, "shot.png", (40, 40)).as_path());
        let mut thumbnails = Thumbnails::default();
        thumbnails.set_protocol(Protocol::Iterm2);
        thumbnails.set_cell(Some(CELL));
        let at = |y| Placement {
            path: Arc::clone(&path),
            x: 3,
            y,
            cols: 4,
            rows: 2,
        };

        let first = String::from_utf8(thumbnails.encode(&[at(5)])).unwrap();
        assert!(
            first.contains("\x1b[6;4H\x1b]1337;File=inline=1;"),
            "drawn at the reserved cells: {first:?}"
        );
        assert!(first.contains(";width=4;height=2;"));
        assert!(!first.contains("\x1b_G"), "no Kitty commands");
        assert!(!thumbnails.take_repaint(), "nothing to erase yet");
        assert!(thumbnails.encode(&[at(5)]).is_empty(), "unchanged frame");

        // A full repaint wiped the cells: draw again, no extra repaint.
        thumbnails.invalidate();
        assert!(thumbnails.stale());
        let again = String::from_utf8(thumbnails.encode(&[at(5)])).unwrap();
        assert!(again.contains("\x1b[6;4H\x1b]1337;File="));
        assert!(!thumbnails.stale() && !thumbnails.take_repaint());

        // Scrolling moves it: repaint to erase the old cells, then draw.
        let moved = String::from_utf8(thumbnails.encode(&[at(7)])).unwrap();
        assert!(moved.contains("\x1b[8;4H\x1b]1337;File="));
        assert!(thumbnails.take_repaint());

        // Covered or scrolled away: no bytes, but the cells are repainted.
        assert!(thumbnails.encode(&[]).is_empty());
        assert!(thumbnails.take_repaint());
        thumbnails.invalidate();
        assert!(!thumbnails.stale(), "nothing shown, nothing to redraw");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
