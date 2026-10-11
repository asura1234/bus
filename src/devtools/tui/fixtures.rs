//! `fixture`: files to send through a scratch Bus. Images easy to recognise in
//! a screenshot (four coloured quadrants, a red diagonal, a blue circle), a
//! text file, and the image the clipboard stand-in hands to Bus's Ctrl+V.
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
    pattern: Option<String>,
) -> Result<Value, String> {
    let pattern = Pattern::parse(pattern.as_deref())?;
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
            (path.clone(), write_image(&path, pattern)?)
        }
        "clipboard-image" => {
            if name.is_some() {
                return Err("clipboard-image takes no --file".into());
            }
            let path = paths.clipboard_image();
            (path.clone(), write_image(&path, pattern)?)
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

#[derive(Clone, Copy)]
enum Pattern {
    Quadrants,
    RedDiagonal,
    BlueCircle,
}

const WHITE: [u8; 3] = [245, 245, 245];

impl Pattern {
    fn parse(name: Option<&str>) -> Result<Self, String> {
        match name.unwrap_or("quadrants") {
            "quadrants" => Ok(Self::Quadrants),
            "red-diagonal" => Ok(Self::RedDiagonal),
            "blue-circle" => Ok(Self::BlueCircle),
            other => Err(format!(
                "unknown --pattern {other:?}; use quadrants, red-diagonal or blue-circle"
            )),
        }
    }

    fn color(self, x: u32, y: u32) -> [u8; 3] {
        let (fx, fy) = (x as f32 / IMAGE_W as f32, y as f32 / IMAGE_H as f32);
        match self {
            Self::Quadrants => {
                let quadrant = match (x < IMAGE_W / 2, y < IMAGE_H / 2) {
                    (true, true) => 0,
                    (false, true) => 1,
                    (false, false) => 2,
                    (true, false) => 3,
                };
                QUADRANTS[quadrant]
            }
            // A thick red band from the top left to the bottom right on white.
            Self::RedDiagonal if (fx - fy).abs() < 0.15 => QUADRANTS[0],
            // A filled blue circle in the middle on white.
            Self::BlueCircle
                if ((fx - 0.5) * IMAGE_W as f32).powi(2)
                    + ((fy - 0.5) * IMAGE_H as f32).powi(2)
                    < (0.4 * IMAGE_H as f32).powi(2) =>
            {
                QUADRANTS[1]
            }
            Self::RedDiagonal | Self::BlueCircle => WHITE,
        }
    }
}

fn write_image(path: &Path, pattern: Pattern) -> Result<u64, String> {
    let mut pixels = Vec::with_capacity((IMAGE_W * IMAGE_H * 3) as usize);
    for y in 0..IMAGE_H {
        for x in 0..IMAGE_W {
            pixels.extend_from_slice(&pattern.color(x, y));
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
