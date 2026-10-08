use super::placement::ClippedPlacement;
use base64::Engine;
use std::fmt::Write as FmtWrite;
use std::io::Write;
pub(crate) const KITTY_CHUNK_BYTES: usize = 3072;

pub(crate) fn encode_delete_image(out: &mut Vec<u8>, id: u32) {
    let _ = write!(out, "\x1b_Ga=d,d=I,i={id},q=2;\x1b\\");
}

pub(crate) fn encode_delete_placement(out: &mut Vec<u8>, host_id: u32, host_placement_id: u32) {
    let _ = write!(
        out,
        "\x1b_Ga=d,d=i,i={host_id},p={host_placement_id},q=2;\x1b\\"
    );
}

pub(crate) fn encode_display_placement(
    out: &mut Vec<u8>,
    clipped: ClippedPlacement,
    host_id: u32,
    host_placement_id: u32,
    z: i32,
) {
    let _ = write!(out, "\x1b[{};{}H", clipped.y + 1, clipped.x + 1);
    let mut control = format!(
        "a=p,i={host_id},p={host_placement_id},c={},r={},z={z},C=1,q=2",
        clipped.cols, clipped.rows,
    );
    append_placement_controls(&mut control, clipped);
    let _ = write!(out, "\x1b_G{control};\x1b\\");
}

fn append_placement_controls(control: &mut String, clipped: ClippedPlacement) {
    if clipped.source_x > 0 {
        let _ = write!(control, ",x={}", clipped.source_x);
    }
    if clipped.source_y > 0 {
        let _ = write!(control, ",y={}", clipped.source_y);
    }
    if clipped.source_width > 0 {
        let _ = write!(control, ",w={}", clipped.source_width);
    }
    if clipped.source_height > 0 {
        let _ = write!(control, ",h={}", clipped.source_height);
    }
    if clipped.x_offset > 0 {
        let _ = write!(control, ",X={}", clipped.x_offset);
    }
    if clipped.y_offset > 0 {
        let _ = write!(control, ",Y={}", clipped.y_offset);
    }
}

pub(crate) fn encode_kitty_data(out: &mut Vec<u8>, control: &str, data: &[u8]) {
    let mut chunks = data.chunks(KITTY_CHUNK_BYTES).peekable();
    let Some(first) = chunks.next() else {
        return;
    };
    let more = if chunks.peek().is_some() { 1 } else { 0 };
    let encoded = base64::engine::general_purpose::STANDARD.encode(first);
    let _ = write!(out, "\x1b_G{control},m={more};{encoded}\x1b\\");

    while let Some(chunk) = chunks.next() {
        let more = if chunks.peek().is_some() { 1 } else { 0 };
        let encoded = base64::engine::general_purpose::STANDARD.encode(chunk);
        let _ = write!(out, "\x1b_Gm={more};{encoded}\x1b\\");
    }
}
