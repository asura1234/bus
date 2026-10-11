use super::*;

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).to_owned()).collect()
}

#[test]
fn osascript_notifications_are_recorded_with_their_title_and_body() {
    let (effect, code) = osascript(&args(&[
        "-e",
        "on run argv",
        "-e",
        "display notification (item 2 of argv) with title (item 1 of argv)",
        "-e",
        "end run",
        "Bus",
        "a1 finished",
    ]));
    assert_eq!(code, 0);
    assert_eq!(effect["kind"], "notification");
    assert_eq!(effect["target"], "Bus");
    assert_eq!(effect["body"], "a1 finished");
}

#[test]
fn clipboard_image_reads_name_their_output_file() {
    let script = "set png_data to (the clipboard as «class PNGf»)\nset fp to open for access POSIX file \"/tmp/x y.png\" with write permission";
    assert_eq!(posix_file(script), Some(PathBuf::from("/tmp/x y.png")));
    let (effect, _) = osascript(&args(&["-e", "tell app \"Finder\" to activate"]));
    assert_eq!(effect["kind"], "osascript");
}

#[test]
fn the_log_reads_back_in_order_and_matches_by_kind_and_target() {
    let log = crate::utils::test_temp::unique_temp_path("effects");
    append(&log, &json!({"kind": "sound", "target": "/tmp/ding.mp3"}));
    append(
        &log,
        &json!({"kind": "open", "target": "https://example.com/docs"}),
    );
    let recorded = read(&log);
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[1]["index"], 1);
    assert!(matches(&recorded[1], "open", Some("example.com")));
    assert!(!matches(&recorded[1], "open", Some("other.org")));
    assert!(matches(&recorded[0], "sound", None));
    std::fs::remove_file(log).unwrap();
}

#[test]
fn terminal_notifications_are_found_across_chunks() {
    let mut watch = OscWatch::default();
    assert!(watch.feed(b"text \x1b]9;bu").is_empty());
    assert_eq!(
        watch.feed(b"ild done\x07 \x1b]0;title\x07"),
        vec![("9".into(), "build done".into())]
    );
    assert_eq!(
        watch.feed(b"\x1b]777;notify;T;B\x1b\\"),
        vec![("777".into(), "notify;T;B".into())]
    );
    assert!(watch.feed(b"\x1b").is_empty());
    assert_eq!(
        watch.feed(b"]99;;hi\x07"),
        vec![("99".into(), ";hi".into())]
    );
}

#[test]
fn a_notification_cut_short_by_another_sequence_is_seen_and_stripped_alike() {
    // An ESC that is not ST cancels the string and starts the next sequence.
    let bytes = b"a\x1b]9;done\x1b[31mred";
    assert_eq!(
        OscWatch::default().feed(bytes),
        vec![("9".into(), "done".into())]
    );
    assert_eq!(
        crate::devtools::tui::mirror::strip_effects(bytes),
        b"a\x1b[31mred".to_vec()
    );
}
