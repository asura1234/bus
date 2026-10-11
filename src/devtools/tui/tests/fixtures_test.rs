use super::*;

#[test]
fn the_image_quadrants_run_red_blue_yellow_cyan_clockwise_from_the_top_left() {
    let path = crate::utils::test_temp::unique_temp_path("fixture-quadrants").with_extension("png");
    write_image(&path).unwrap();
    let decoder = png::Decoder::new(std::fs::File::open(&path).unwrap());
    let mut reader = decoder.read_info().unwrap();
    let mut data = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut data).unwrap();
    std::fs::remove_file(&path).unwrap();
    let (w, h) = (info.width as usize, info.height as usize);
    let pixel = |x: usize, y: usize| {
        let at = (y * w + x) * 3;
        [data[at], data[at + 1], data[at + 2]]
    };
    let (red, blue, yellow, cyan) = ([220, 40, 40], [40, 90, 220], [230, 210, 40], [40, 200, 220]);
    assert_eq!(pixel(0, 0), red, "top left");
    assert_eq!(pixel(w - 1, 0), blue, "top right");
    assert_eq!(pixel(w - 1, h - 1), yellow, "bottom right");
    assert_eq!(pixel(0, h - 1), cyan, "bottom left");
}
