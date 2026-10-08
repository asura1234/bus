use super::*;

fn info_header(width: i32, height: i32, bit_count: u16, compression: u32) -> Vec<u8> {
    let mut header = vec![0_u8; 40];
    header[0..4].copy_from_slice(&40_u32.to_le_bytes());
    header[4..8].copy_from_slice(&width.to_le_bytes());
    header[8..12].copy_from_slice(&height.to_le_bytes());
    header[12..14].copy_from_slice(&1_u16.to_le_bytes());
    header[14..16].copy_from_slice(&bit_count.to_le_bytes());
    header[16..20].copy_from_slice(&compression.to_le_bytes());
    header
}

fn decode_rgba(bytes: &[u8]) -> Vec<u8> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().unwrap();
    let mut output = vec![0_u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut output).unwrap();
    output.truncate(info.buffer_size());
    output
}

#[test]
fn converts_bottom_up_24_bit_dib_to_png() {
    let mut dib = info_header(2, 2, 24, BI_RGB);
    dib.extend_from_slice(&[
        255, 0, 0, 255, 255, 255, 0, 0, // bottom: blue, white, padding
        0, 0, 255, 0, 255, 0, 0, 0, // top: red, green, padding
    ]);

    let png = dib_to_png(&dib, 16 * 1024 * 1024).unwrap();
    assert_eq!(dib_to_png(&dib, png.len()), Some(png.clone()));
    assert!(dib_to_png(&dib, png.len() - 1).is_none());
    assert_eq!(
        decode_rgba(&png),
        vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,]
    );
}

#[test]
fn strips_registered_png_allocator_tail() {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&[1, 2, 3, 255]).unwrap();
    writer.finish().unwrap();
    let expected = bytes.clone();
    bytes.extend_from_slice(&[0; 32]);

    assert_eq!(
        validated_png(&bytes, expected.len()),
        Some(expected.clone())
    );
    assert!(validated_png(&bytes, expected.len() - 1).is_none());
    assert_eq!(validated_png(&bytes, 16 * 1024 * 1024), Some(expected));
}
