use super::*;

const LOGIN_ERROR: &str =
    include_str!("../../../../tests/fixtures/grok-login/native-1.0.70-error.txt");
const CONNECTING: &str =
    include_str!("../../../../tests/fixtures/grok-login/native-1.0.70-connecting.txt");
const NARROW_ERROR: &str =
    include_str!("../../../../tests/fixtures/grok-login/native-1.0.70-error-71col.txt");
const NARROW_REAL_HOME_ERROR: &str =
    include_str!("../../../../tests/fixtures/grok-login/native-1.0.70-error-61col.txt");
const IDLE: &str = include_str!("../../../../tests/fixtures/grok-composer/native-1.0.70-empty.txt");

#[test]
fn captured_grok_login_error_blocks_despite_idle_title_and_composer() {
    assert!(LOGIN_ERROR.contains("Login with Grok              l"));
    assert!(LOGIN_ERROR.contains("❯ Type a message..."));
    assert_login_blocked(LOGIN_ERROR);
    let dialog = crate::agents::dialog::parse(LOGIN_ERROR).unwrap();
    assert_eq!(dialog.keys_for(1), Some(vec!["l"]));
    assert_eq!(dialog.keys_for(2), Some(vec!["q"]));
}

#[test]
fn captured_grok_login_connecting_blocks_despite_idle_title() {
    assert!(CONNECTING.contains("Connecting..."));
    assert_login_blocked(CONNECTING);
    let dialog = crate::agents::dialog::parse(CONNECTING).unwrap();
    assert!(dialog.options.is_empty());
    assert!(dialog.keys_for(1).is_none());
}

#[test]
fn captured_grok_login_dialogs_explain_the_blocker() {
    for screen in [LOGIN_ERROR, CONNECTING] {
        let dialog = crate::agents::dialog::parse(screen).expect("login must be inspectable");
        assert!(dialog.text.contains("Grok login required"), "{dialog:?}");
    }
}

#[test]
fn captured_wrapped_grok_login_error_blocks_in_a_narrow_pane() {
    assert!(
        NARROW_ERROR.contains("openid-\n"),
        "fixture lost its native wrap"
    );
    assert!(NARROW_REAL_HOME_ERROR.contains("well-known/\n"));
    for screen in [NARROW_ERROR, NARROW_REAL_HOME_ERROR] {
        assert_login_blocked(screen);
    }
}

fn assert_login_blocked(screen: &str) {
    for (title, progress) in [("grok", "4;0;0"), ("", "")] {
        let result = detect_screen_with_osc(AgentKind::Grok, screen, title, progress);
        assert_eq!(result.state, AgentState::Blocked, "{result:?}");
        assert!(result.visible_blocker, "{result:?}");
        assert!(!result.visible_idle, "{result:?}");
        assert_eq!(
            result.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("login_unavailable")
        );
    }
    let dialog = crate::agents::dialog::parse(screen).expect("login reason must be inspectable");
    assert!(dialog.text.contains("Grok login required"), "{dialog:?}");
}

#[test]
fn captured_authenticated_grok_idle_remains_idle() {
    let result = detect_screen_with_osc(AgentKind::Grok, IDLE, "grok", "4;0;0");
    assert_eq!(result.state, AgentState::Idle, "{result:?}");
    assert!(result.visible_idle);
    assert!(!result.visible_blocker);
}

#[test]
fn grok_conversation_quoting_login_controls_is_not_blocked() {
    for output in [
        "The login screen says Login with Grok; choose it to sign in.",
        "Login with Grok",
        "Login with Grok              l\nQuit                         q",
        "Connecting...\nctrl+q  quit",
        LOGIN_ERROR,
        CONNECTING,
        NARROW_ERROR,
        NARROW_REAL_HOME_ERROR,
    ] {
        let frame = &IDLE[IDLE.find("  ╭").unwrap()..];
        let conversation = format!("  /project\n\n❯ Explain the login screen\n\n{output}\n{frame}");
        let result = detect_screen_with_osc(AgentKind::Grok, &conversation, "grok", "4;0;0");
        assert_eq!(result.state, AgentState::Idle, "{output:?}: {result:?}");
        assert!(!result.visible_blocker);
        assert!(crate::agents::dialog::parse(&conversation).is_none());

        // Transcript scrolling can put a quoted label at the viewport top.
        let quoted_at_top = format!("{output}\n{frame}");
        let result = detect_screen_with_osc(AgentKind::Grok, &quoted_at_top, "grok", "4;0;0");
        assert_eq!(result.state, AgentState::Idle, "{output:?}: {result:?}");
        assert!(crate::agents::dialog::parse(&quoted_at_top).is_none());
    }
}
