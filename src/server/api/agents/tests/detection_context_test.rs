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
