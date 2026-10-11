//! `fixture`: files to send through a scratch Bus. An image whose four
//! coloured quadrants are easy to recognise in a screenshot, a text file, and
//! the image the clipboard stand-in hands to Bus's Ctrl+V image paste.
use super::session::SessionPaths;
use serde_json::{json, Value};
use std::path::Path;

/// Red, blue, yellow and cyan quadrants, clockwise from the top left.
const QUADRANTS: [[u8; 3]; 4] = [[220, 40, 40], [40, 90, 220], [230, 210, 40], [40, 200, 220]];
const IMAGE_W: u32 = 160;
const IMAGE_H: u32 = 100;

pub(super) fn create(
    paths: &SessionPaths,
    kind: &str,
    name: Option<String>,
    text: Option<String>,
) -> Result<Value, String> {
    if name
        .as_deref()
        .is_some_and(|n| n.is_empty() || n.starts_with('.') || n.contains(['/', '\\']))
    {
        return Err("--file must be a plain file name".into());
    }
    let (path, size) = match kind {
        "image" => {
            let path = paths
                .work_dir()
                .join(name.unwrap_or_else(|| "fixture-image.png".into()));
            (path.clone(), write_image(&path)?)
        }
        "clipboard-image" => {
            if name.is_some() {
                return Err("clipboard-image takes no --file".into());
            }
            let path = paths.clipboard_image();
            (path.clone(), write_image(&path)?)
        }
        "file" => {
            let path = paths
                .work_dir()
                .join(name.unwrap_or_else(|| "fixture-notes.txt".into()));
            let text = text.unwrap_or_else(|| "Fixture file for a Bus driver session.\n".into());
            std::fs::write(&path, &text).map_err(|e| format!("{}: {e}", path.display()))?;
            (path, text.len() as u64)
        }
        _ => {
            return Err(format!(
                "unknown fixture {kind:?}; use image, file or clipboard-image"
            ))
        }
    };
    Ok(json!({"ok": true, "kind": kind, "path": path, "bytes": size}))
}

fn write_image(path: &Path) -> Result<u64, String> {
    let mut pixels = Vec::with_capacity((IMAGE_W * IMAGE_H * 3) as usize);
    for y in 0..IMAGE_H {
        for x in 0..IMAGE_W {
            let quadrant = match (x < IMAGE_W / 2, y < IMAGE_H / 2) {
                (true, true) => 0,
                (false, true) => 1,
                (false, false) => 2,
                (true, false) => 3,
            };
            pixels.extend_from_slice(&QUADRANTS[quadrant]);
        }
    }
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, IMAGE_W, IMAGE_H);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer
        .write_image_data(&pixels)
        .map_err(|e| e.to_string())?;
    drop(writer);
    std::fs::write(path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(bytes.len() as u64)
}

#[cfg(test)]
#[path = "tests/fixtures_test.rs"]
mod tests;
