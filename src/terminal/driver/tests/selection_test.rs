use super::*;

#[test]
fn select_distinguishes_inline_buttons_from_ambiguous_menu_options() {
    let screen =
        include_str!("../../../../tests/fixtures/grok-composer/native-1.0.70-privacy-banner.txt");
    let error = selection_step(&menu(screen), "Opt out").unwrap_err();
    assert!(error.contains("No complete menu row"), "{error}");
    assert!(
        !error.contains("ambiguous"),
        "an absent menu row is not a duplicate option: {error}"
    );
    let duplicate = selection_step(&menu("❯ Opt out\n  Opt out\n  Opt in"), "Opt out").unwrap_err();
    assert!(duplicate.contains("ambiguous"), "{duplicate}");
    assert_eq!(
        selection_step(&menu("❯ Opt out\n  Opt in"), "Opt out"),
        Ok(SelectionStep::Confirm)
    );
    assert!(selection_step(&menu("❯ Opt out for this session\n  Opt in"), "Opt out").is_err());
    assert!(selection_step(&menu("❯ Yes, once\n  Yes, always"), "Yes").is_err());
}
