use super::*;

#[test]
fn ctrl_e_moves_to_line_end_and_shift_toggles_composer_size() {
    let (mut ui, room, _) = fixture();
    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert("hello\nworld");
    ui.locals.get_mut(&room).unwrap().text.cursor = 0;
    key(&mut ui, KeyCode::Char('e'), KeyModifiers::CONTROL);
    assert_eq!(ui.locals[&room].text.cursor, 5);
    assert_eq!(ui.locals[&room].composer_size, ComposerSize::Auto);
    key(
        &mut ui,
        KeyCode::Char('e'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    assert_eq!(ui.locals[&room].composer_size, ComposerSize::Full);
}

#[test]
fn ctrl_r_searches_history_and_shift_opens_a_room_form() {
    let (mut ui, room, _) = fixture();
    ui.locals.get_mut(&room).unwrap().recall = vec!["alpha path".into(), "beta note".into()];
    key(&mut ui, KeyCode::Char('r'), KeyModifiers::CONTROL);
    assert!(ui.form.is_none());
    assert_eq!(ui.locals[&room].text.text, "beta note");
    key(&mut ui, KeyCode::Char('p'), KeyModifiers::NONE);
    assert_eq!(ui.locals[&room].text.text, "alpha path");
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(ui.history_search.is_none());
    assert_eq!(ui.locals[&room].text.text, "alpha path");
    key(
        &mut ui,
        KeyCode::Char('r'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    assert!(matches!(ui.form, Some(forms::Form::Room(_))));
}

#[test]
fn up_browses_recall_and_esc_esc_saves_the_draft() {
    let (mut ui, room, _) = fixture();
    ui.locals.get_mut(&room).unwrap().recall = vec!["older".into(), "newer".into()];
    ui.locals.get_mut(&room).unwrap().text.insert("live");
    key(&mut ui, KeyCode::Up, KeyModifiers::NONE);
    assert_eq!(ui.locals[&room].text.text, "newer");
    key(&mut ui, KeyCode::Up, KeyModifiers::NONE);
    assert_eq!(ui.locals[&room].text.text, "older");
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(ui.locals[&room].text.text, "live");
    key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);
    assert!(ui.locals[&room].text.text.is_empty());
    assert_eq!(
        ui.locals[&room].recall.last().map(String::as_str),
        Some("live")
    );
    key(&mut ui, KeyCode::Up, KeyModifiers::NONE);
    assert_eq!(ui.locals[&room].text.text, "live");
}

#[test]
fn stash_backslash_enter_and_ctrl_g_use_composer_chords() {
    let (mut ui, room, _) = fixture();
    ui.locals.get_mut(&room).unwrap().text.insert("parked");
    key(&mut ui, KeyCode::Char('s'), KeyModifiers::CONTROL);
    assert!(ui.locals[&room].text.text.is_empty());
    key(&mut ui, KeyCode::Char('s'), KeyModifiers::CONTROL);
    assert_eq!(ui.locals[&room].text.text, "parked");
    ui.locals.get_mut(&room).unwrap().text = Default::default();
    key(&mut ui, KeyCode::Char('\\'), KeyModifiers::NONE);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(ui.locals[&room].text.text, "\n");
    assert!(ui.send_intent.is_none());
    let mut outcome = crate::client::shell::ClientShellInput::default();
    ui.input(
        &RawInputEvent::Key(TerminalKey::new(KeyCode::Char('g'), KeyModifiers::CONTROL)),
        false,
        &mut outcome,
    );
    assert!(outcome.actions.iter().any(|action| matches!(
        action,
        crate::client::shell::ClientShellAction::EditComposer
    )));
}

#[test]
fn ctrl_v_does_not_insert_a_letter() {
    let (mut ui, room, _) = fixture();
    key(&mut ui, KeyCode::Char('v'), KeyModifiers::CONTROL);
    assert!(ui.locals[&room].text.text.is_empty());
}

#[test]
fn apply_external_edit_replaces_the_draft() {
    let (mut ui, room, _) = fixture();
    ui.locals.get_mut(&room).unwrap().text.insert("before");
    ui.apply_external_edit(room, "from editor".into());
    assert_eq!(ui.locals[&room].text.text, "from editor");
}
