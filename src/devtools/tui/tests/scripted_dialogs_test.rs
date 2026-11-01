use super::screens::{Interaction, ScreenInput};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::empty()))
}

fn input(value: serde_json::Value) -> ScreenInput {
    serde_json::from_value(value).unwrap()
}

#[test]
fn scripted_screen_menu_does_not_accept_the_initial_install_row() {
    let mut prompt = Interaction::new(input(serde_json::json!({
        "mode":"menu", "labels":["Install update now","Continue with current version"],
        "answer":1
    })));
    assert!(prompt.render("{{options}}").starts_with("› 1. Install"));
    assert_eq!(
        prompt.apply(key(KeyCode::Enter)),
        Some(("Install update now".into(), false))
    );
    assert_eq!(prompt.apply(key(KeyCode::Down)), None);
    assert!(prompt.render("{{options}}").contains("› 2. Continue"));
    assert_eq!(
        prompt.apply(key(KeyCode::Enter)),
        Some(("Continue with current version".into(), true))
    );
}

#[test]
fn scripted_screen_text_supports_paste_unicode_and_editing_before_submit() {
    let mut prompt = Interaction::new(input(
        serde_json::json!({"mode":"text","answer":"ANSI 屏幕"}),
    ));
    assert_eq!(prompt.apply(Event::Paste("ANSI 屏X".into())), None);
    assert_eq!(prompt.apply(key(KeyCode::Backspace)), None);
    assert_eq!(prompt.apply(key(KeyCode::Char('幕'))), None);
    assert_eq!(prompt.render("Answer: {{answer}}"), "Answer: ANSI 屏幕");
    assert_eq!(
        prompt.apply(key(KeyCode::Enter)),
        Some(("ANSI 屏幕".into(), true))
    );
}

#[test]
fn scripted_screen_number_and_control_chord_complete_without_enter() {
    let mut number = Interaction::new(input(serde_json::json!({"mode":"number","answer":"2"})));
    assert_eq!(
        number.apply(key(KeyCode::Char('2'))),
        Some(("2".into(), true))
    );
    let mut chord = Interaction::new(input(serde_json::json!({"mode":"key","answer":"ctrl+l"})));
    assert_eq!(
        chord.apply(Event::Key(KeyEvent::new(
            KeyCode::Char('l'),
            KeyModifiers::CONTROL
        ))),
        Some(("ctrl+l".into(), true))
    );
}

#[test]
fn scripted_screen_invalid_menu_indexes_are_rejected_before_showing_prompt() {
    let invalid = input(serde_json::json!({"mode":"menu","labels":["Continue"],"answer":1}));
    assert!(invalid.validate("{{options}}").is_err());
    let invalid = input(serde_json::json!({"mode":"number","answer":"12"}));
    assert!(invalid.validate("Select a number").is_err());
}

#[test]
fn scripted_dialog_suite_loads_fifteen_distinct_screen_shapes() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/dialog-driver/suite.json");
    let temp = tempfile::tempdir().unwrap();
    let mut scenario =
        super::script::Scenario::load(&fixture, temp.path().join("state.json")).unwrap();
    let actions = scenario.actions("RUN_DIALOG_DRIVER_SUITE").unwrap();
    let ids = actions
        .iter()
        .filter_map(|action| match action {
            super::script::Action::Screen { id, .. } => Some(id),
            _ => None,
        })
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(ids.len(), 15);
    assert!(ids.iter().any(|id| id.as_str() == "oh-my-zsh-update"));
    assert!(matches!(
        actions.last(),
        Some(super::script::Action::Reply { .. })
    ));
}
