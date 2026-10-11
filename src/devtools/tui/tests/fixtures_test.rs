use super::*;

#[test]
fn the_image_quadrants_run_red_blue_yellow_cyan_clockwise_from_the_top_left() {
    let path = crate::utils::test_temp::unique_temp_path("fixture-quadrants").with_extension("png");
    write_image(&path, Pattern::Quadrants).unwrap();
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

#[test]
fn the_other_patterns_are_a_red_diagonal_and_a_blue_circle_on_white() {
    let (red, blue, white) = ([220, 40, 40], [40, 90, 220], [245, 245, 245]);
    // The top left corner, the middle and the top right corner.
    let points = [(0, 0), (IMAGE_W / 2, IMAGE_H / 2), (IMAGE_W - 1, 0)];
    for (pattern, expected) in [
        (Pattern::RedDiagonal, [red, red, white]),
        (Pattern::BlueCircle, [white, blue, white]),
    ] {
        for ((x, y), color) in points.into_iter().zip(expected) {
            assert_eq!(pattern.color(x, y), color, "({x}, {y})");
        }
    }
}

#[test]
fn each_fixture_kind_takes_only_its_own_flags() {
    let paths =
        super::super::session::SessionPaths::new(std::path::Path::new("/nonexistent"), "s1");
    let refused = |kind: &str, file: Option<&str>, text: Option<&str>, pattern: Option<&str>| {
        create(
            &paths,
            kind,
            file.map(Into::into),
            text.map(Into::into),
            pattern.map(Into::into),
        )
        .unwrap_err()
    };
    assert!(refused("clipboard-image", Some("x.png"), None, None)
        .starts_with("clipboard-image takes no --file"));
    assert!(refused("clipboard-image", None, Some("t"), None)
        .starts_with("clipboard-image takes no --text"));
    assert!(refused("image", None, Some("t"), None).starts_with("image takes no --text"));
    assert!(refused("file", None, None, Some("blue-circle")).starts_with("file takes no --pattern"));
    // The usage line offers each kind only what it takes.
    assert!(USAGE.contains("clipboard-image [--pattern P]"));
    assert!(!USAGE.contains("clipboard-image [--file"));
    let doc = include_str!("../../../../docs/tui-driver.md");
    assert!(
        doc.contains(USAGE.trim_start_matches("usage: ")),
        "docs/tui-driver.md fixture line"
    );
}
