use super::*;
use crate::protocol::api::schema::{
    AgentDetectionContext, AgentDetectionContexts, Method, Request,
};

const UPDATE: &str = "New version available: 2.4.1\n❯ Install update now\n  Continue with current version\n↑/↓ to select, Enter to confirm";
const DRAFT: &str =
    "╭────────────────────────╮\n│ > developer draft      │\n╰────────────────────────╯";
const EMPTY: &str =
    "╭────────────────────────╮\n│ >                      │\n╰────────────────────────╯";

async fn detect(
    kind: AgentKind,
    screen: &str,
    obligation: bool,
    age: Option<u64>,
    wrong_identity: bool,
) -> AgentState {
    let (mut app, mut writes) =
        app_with_dialog_sized(screen.replace('\n', "\r\n").as_bytes(), None, 180, 65);
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let terminal = app.state.workspaces[0].tabs[0].panes[&pane]
        .attached_terminal_id
        .clone();
    {
        let state = app.state.terminals.get_mut(&terminal).unwrap();
        state.set_detected_state(Some(kind), AgentState::Idle);
        state.set_agent_name("reviewer".into());
    }
    let info = app.agent_info(0, pane).unwrap();
    let response = app.handle_api_request(Request {
        id: "detection-context".into(),
        method: Method::AgentListWithDetectionContext(AgentDetectionContexts {
            agents: vec![AgentDetectionContext {
                terminal_id: if wrong_identity {
                    "replaced-terminal".into()
                } else {
                    info.terminal_id
                },
                pane_id: info.pane_id,
                session_id: None,
                obligation,
                activity_age_ms: age,
            }],
        }),
    });
    let parsed: SuccessResponse = serde_json::from_str(&response).unwrap();
    assert!(matches!(parsed.result, ResponseResult::AgentList { .. }));
    let runtime = app.lookup_runtime_sender(0, pane).unwrap();
    let (state, _) = runtime
        .test_scan_detection_context(kind, AgentState::Unknown)
        .await;
    app.state
        .terminals
        .get_mut(&terminal)
        .unwrap()
        .set_detected_state(Some(kind), state);
    let info = app.agent_info(0, pane).unwrap();
    if screen == UPDATE {
        assert!(
            info.dialog_id.is_some(),
            "visible generic controls have a driver episode ID"
        );
    }
    assert!(
        writes.try_recv().is_err(),
        "state detection must send no key"
    );
    state
}

#[tokio::test]
async fn native_controls_block_without_obligation_and_busy_drafts_require_one() {
    for screen in [UPDATE, DRAFT] {
        assert_eq!(
            detect(AgentKind::Kimi, screen, true, None, false).await,
            AgentState::Blocked
        );
        assert_eq!(
            detect(AgentKind::Kimi, screen, false, None, false).await,
            if screen == UPDATE {
                AgentState::Blocked
            } else {
                AgentState::Idle
            }
        );
    }
    assert_eq!(
        detect(AgentKind::Kimi, EMPTY, true, None, false).await,
        AgentState::Idle
    );
    assert_eq!(
        detect(AgentKind::Kimi, UPDATE, true, None, true).await,
        AgentState::Blocked
    );
}

#[tokio::test]
async fn native_context_keeps_recent_hook_activity_working_but_expires_it() {
    assert_eq!(
        detect(AgentKind::Kimi, UPDATE, true, Some(0), false).await,
        AgentState::Working
    );
    assert_eq!(
        detect(AgentKind::Kimi, UPDATE, true, Some(1000), false).await,
        AgentState::Blocked
    );
    assert_eq!(
        detect(AgentKind::Kimi, EMPTY, true, Some(0), false).await,
        AgentState::Idle
    );
}

const CLAUDE_IDLE: &str = include_str!("fixtures/claude_2_1_310_idle_88cols.txt");

async fn detect_scrolled_claude(live: &str, history: &str) -> AgentState {
    let (runtime, mut writes) =
        crate::terminal::runtime::TerminalRuntime::test_with_channel_and_scrollback_bytes(
            88, 40, 100_000, b"", 16,
        );
    let text = format!(
        "\x1b]0;✳ Claude Code\x07{history}\n{}\n{live}",
        "captured transcript\n".repeat(60)
    );
    runtime.test_process_pty_bytes(text.replace('\n', "\r\n").as_bytes());
    runtime.set_scroll_offset_from_bottom(60);
    assert!(runtime.visible_text().contains("captured transcript"));
    let viewport = runtime.visible_text();
    runtime.set_detection_context(
        AgentKind::Claude,
        true,
        None,
        crate::server::api::agents::prompt::detection_composer(AgentKind::Claude),
    );
    let state = runtime
        .test_scan_detection_context(AgentKind::Claude, AgentState::Unknown)
        .await
        .0;
    assert!(writes.try_recv().is_err(), "detection must send no keys");
    assert_eq!(
        runtime.visible_text(),
        viewport,
        "preserve the reading position"
    );
    state
}

#[tokio::test]
async fn captured_claude_idle_composer_stays_idle_when_viewport_is_scrolled() {
    // Real 2.1.310, 88 columns: the composer is on the active screen while
    // the developer reads transcript scrollback. OSC still signals idle.
    assert_eq!(
        detect_scrolled_claude(CLAUDE_IDLE, "historical response").await,
        AgentState::Idle
    );
}

#[tokio::test]
async fn scrolled_history_empty_composer_does_not_hide_live_claude_draft() {
    let draft = CLAUDE_IDLE.replacen("\n❯\n", "\n❯ developer draft\n", 1);
    assert_ne!(draft, CLAUDE_IDLE);
    assert_eq!(
        detect_scrolled_claude(&draft, CLAUDE_IDLE).await,
        AgentState::Blocked
    );
}

#[tokio::test]
async fn claude_idle_title_does_not_hide_a_permission_dialog() {
    assert_eq!(
        detect(
            AgentKind::Claude,
            "\x1b]0;✳ Claude Code\x07do you want to proceed?\nbash command: echo test\n❯ 1. Yes\n  2. No\n\nEsc to cancel · Tab to amend · ctrl+e to explain",
            true,
            None,
            false,
        )
        .await,
        AgentState::Blocked
    );
}

#[tokio::test]
async fn unavailable_composer_with_idle_title_remains_guarded() {
    for kind in [AgentKind::Claude, AgentKind::Codex, AgentKind::Kimi] {
        assert_eq!(
            detect(
                kind,
                "\x1b]0;✳ Claude Code\x07No composer",
                true,
                None,
                false
            )
            .await,
            AgentState::Blocked,
            "{kind:?}"
        );
    }
}

#[tokio::test]
async fn codex_and_kimi_empty_composers_and_drafts_keep_their_states() {
    for (kind, empty, draft) in [
        (
            AgentKind::Codex,
            "›\nGPT-6 · /repo\n? for shortcuts",
            "› developer draft\nGPT-6 · /repo\n? for shortcuts",
        ),
        (AgentKind::Kimi, EMPTY, DRAFT),
    ] {
        assert_eq!(
            detect(kind, empty, true, None, false).await,
            AgentState::Idle,
            "{kind:?}"
        );
        assert_eq!(
            detect(kind, draft, true, None, false).await,
            AgentState::Blocked,
            "{kind:?}"
        );
    }
}

#[tokio::test]
async fn grok_welcome_without_composer_blocks_but_empty_native_composer_is_ready() {
    let welcome =
        include_str!("../../../../../tests/fixtures/grok-composer/native-1.0.70-welcome.txt");
    let card = &welcome[..welcome.rfind("  ╭").unwrap()];
    assert_eq!(
        detect(AgentKind::Grok, card, true, None, false).await,
        AgentState::Blocked
    );
    assert_eq!(
        detect(AgentKind::Grok, welcome, true, None, false).await,
        AgentState::Idle
    );
}

#[tokio::test]
#[cfg(feature = "dev-tools")]
async fn a_provider_without_observed_submission_keeps_its_visible_idle_signal() {
    assert_eq!(
        detect(AgentKind::Fake, "fake> ", true, None, false).await,
        AgentState::Idle
    );
    assert_eq!(
        detect(AgentKind::Fake, "Choose the next action", true, None, false).await,
        AgentState::Blocked
    );
}

#[tokio::test]
async fn codex_runtime_keeps_stale_question_activity_working() {
    for (activity, hint) in [
        ("• Working (12s • esc to interrupt)", "    shift+left to answer"),
        (
            "• Waiting for background terminal (39m 17s • esc to interrupt) · 1 background terminal",
            "    shift+← to answer",
        ),
    ] {
        let screen = format!(
            "{activity}\n\n• Queued follow-up inputs\n  ? 1 question\n{hint}\n\n› Ask Codex to do anything\n"
        );
        assert_eq!(
            detect(AgentKind::Codex, &screen, false, None, false).await,
            AgentState::Working,
            "a collapsed stale question must not replace visible ongoing activity: {screen}"
        );
    }
}
