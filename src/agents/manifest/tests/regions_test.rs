use super::*;
use crate::agents::manifest::regions::validate_region_name;

#[test]
fn bottom_non_empty_lines_uses_bottom_occurrence_for_repeated_text() {
    let content = "marker\nold\n\nmiddle\nmarker\nnew\n";

    assert_eq!(
        region(
            DetectionInput {
                screen: content,
                osc_title: "",
                osc_progress: "",
            },
            "bottom_non_empty_lines(2)"
        ),
        "marker\nnew\n"
    );
}

#[test]
fn top_non_empty_lines_uses_top_occurrence_for_repeated_text() {
    let content = "\nmarker\nold\n\nmiddle\nmarker\nnew\n";

    assert_eq!(
        region(
            DetectionInput {
                screen: content,
                osc_title: "",
                osc_progress: "",
            },
            "top_non_empty_lines(2)"
        ),
        "\nmarker\nold\n"
    );
}

#[test]
fn top_non_empty_lines_requires_a_canonical_positive_bounded_count() {
    let name = "top_non_empty_lines";
    assert!(validate_region_name(&format!("{name}(1)")).is_ok());
    assert!(validate_region_name(&format!("{name}({})", u16::MAX)).is_ok());
    for count in ["0", "01", "+1", "65536", "999999999999999999999999"] {
        assert!(
            validate_region_name(&format!("{name}({count})")).is_err(),
            "{name} accepted invalid count {count}"
        );
    }
}

#[test]
fn top_non_empty_lines_requires_engine_three_when_declared() {
    let manifest = r#"
id = "grok"
version = "1"
min_engine_version = 2

[[rules]]
id = "background"
state = "working"
region = " top_non_empty_lines(1) "
contains = ["active"]
"#;

    assert!(parse_manifest(manifest).is_err());
}

// ---------------------------------------------------------------------------
// OSC rule tests — exercise the new osc_title / osc_progress regions against
// the bundled Claude and Codex manifests.
// ---------------------------------------------------------------------------

#[test]
fn every_region_slices_empty_and_multibyte_screens_on_line_boundaries() {
    let names = [
        "whole_recent",
        "after_last_prompt_marker",
        "before_current_prompt_marker",
        "whole_recent_without_current_prompt_marker",
        "current_prompt_block_marker",
        "after_current_prompt_block_marker",
        "prompt_box_body",
        "above_prompt_box",
        "last_non_empty_above_prompt_box",
        "after_last_horizontal_rule",
        "bottom_lines(2)",
        "bottom_non_empty_lines(2)",
        "top_non_empty_lines(2)",
    ];
    let screen = "• 修复🙂\n─────\n› 输入\n─────\n状态🙂";
    for name in names {
        let input = |screen| DetectionInput {
            screen,
            osc_title: "",
            osc_progress: "",
        };
        assert_eq!(region(input(""), name), "", "{name}");
        let sliced = region(input(screen), name);
        assert!(screen.contains(sliced), "{name}: {sliced:?}");
    }
    let input = DetectionInput {
        screen,
        osc_title: "",
        osc_progress: "",
    };
    assert_eq!(region(input, "prompt_box_body"), "› 输入\n");
    assert_eq!(region(input, "bottom_lines(2)"), "─────\n状态🙂");
    assert_eq!(region(input, "after_last_horizontal_rule"), "状态🙂");
}
