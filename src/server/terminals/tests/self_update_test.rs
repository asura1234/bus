use super::*;
use crate::agents::AgentKind;
use crate::protocol::api::schema;
use crate::server::workspaces::Workspace;

const CHOOSER: &[u8] = include_bytes!("../../../../tests/fixtures/codex-update/chooser-0.162.ansi");
const NAME: &str = "bus-r1-a2";

struct Harness {
    app: App,
    writes: tokio::sync::mpsc::Receiver<Bytes>,
    terminal_id: TerminalId,
    pane: PaneId,
}

/// A Codex agent Bus started with `agent.start`, showing the 0.162 chooser.
fn codex_at_update_chooser() -> Harness {
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(
        &crate::utils::config::Config::default(),
        crate::server::app::AppPolicy::TEST,
        None,
        api_rx,
        crate::server::api::EventHub::default(),
    );
    app.state.workspaces = vec![Workspace::test_new("update")];
    app.state.ensure_test_terminals();
    app.state.active = Some(0);
    app.state.selected = 0;
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane]
        .attached_terminal_id
        .clone();
    let (runtime, mut writes) =
        crate::terminal::TerminalRuntime::test_with_channel_capacity(120, 40, 16);
    app.terminal_runtimes.insert(terminal_id.clone(), runtime);
    let pane_id = app.public_pane_id(0, pane).unwrap();
    let started = app.handle_api_request(schema::Request {
        id: "start".into(),
        method: schema::Method::AgentStart(schema::AgentStartParams {
            name: NAME.into(),
            kind: "codex".into(),
            pane_id,
            args: vec!["-c".into(), "hooks=bus".into()],
            timeout_ms: Some(4_000),
        }),
    });
    assert!(started.contains("agent_started"), "{started}");
    assert!(String::from_utf8_lossy(&writes.try_recv().unwrap()).contains("codex"));
    let mut harness = Harness {
        app,
        writes,
        terminal_id,
        pane,
    };
    harness.show(CHOOSER);
    harness
        .terminal()
        .set_detected_state(Some(AgentKind::Codex), AgentState::Blocked);
    harness
}

impl Harness {
    fn terminal(&mut self) -> &mut crate::terminal::TerminalState {
        self.app.state.terminals.get_mut(&self.terminal_id).unwrap()
    }

    fn show(&self, bytes: &[u8]) {
        self.app
            .terminal_runtimes
            .get(&self.terminal_id)
            .unwrap()
            .test_process_pty_bytes(bytes);
    }

    fn phase(&mut self) -> Option<SelfUpdatePhase> {
        self.terminal()
            .self_update
            .as_ref()
            .map(|update| update.phase.clone())
    }

    /// Codex prints the updater's outcome, exits, and the shell is back.
    fn exit_with(&mut self, output: &str) {
        self.show(format!("\x1b[2J\x1b[H{output}\r\n$ ").as_bytes());
        self.terminal().set_detected_state_with_visible_blocker(
            Some(AgentKind::Codex),
            AgentState::Idle,
            false,
            true,
        );
    }

    fn update_error(&self) -> Option<String> {
        self.app
            .agent_info(0, self.pane)
            .and_then(|info| info.update_error)
    }
}

#[tokio::test]
async fn bus_installs_the_update_and_starts_codex_again_in_the_same_pane() {
    let mut harness = codex_at_update_chooser();
    let now = Instant::now();

    // "Update now" is selected, so Enter alone chooses it, once.
    assert!(harness.app.supervise_self_updates(now));
    assert_eq!(
        harness.writes.try_recv().unwrap(),
        Bytes::from_static(b"\r")
    );
    assert!(matches!(
        harness.phase(),
        Some(SelfUpdatePhase::Installing {
            interrupted: false,
            ..
        })
    ));
    assert!(!harness.app.supervise_self_updates(now));
    assert!(harness.writes.try_recv().is_err());

    harness.exit_with(
        "Updating Codex via `npm install -g @openai/codex`...\r\n\r\n\
         🎉 Update ran successfully! Please restart Codex.",
    );
    assert_eq!(harness.terminal().agent_name, None);
    assert!(harness.app.supervise_self_updates(now));
    let relaunch = String::from_utf8_lossy(&harness.writes.try_recv().unwrap()).into_owned();
    assert!(
        relaunch.contains("codex") && relaunch.contains("hooks=bus"),
        "{relaunch}"
    );
    assert_eq!(harness.terminal().agent_name.as_deref(), Some(NAME));
    assert_eq!(harness.phase(), Some(SelfUpdatePhase::Relaunched));
    assert_eq!(harness.update_error(), None);

    // Once Codex is running again, the update is over.
    harness.show(b"\x1b[2J\x1b[H\xe2\x80\xba Ask Codex to do anything\r\n");
    harness
        .terminal()
        .set_detected_state(Some(AgentKind::Codex), AgentState::Idle);
    harness
        .terminal()
        .reconcile_managed_agent_at(now + std::time::Duration::from_secs(5), false);
    assert!(harness.terminal().managed_agent_interactive_ready());
    assert!(harness.app.supervise_self_updates(now));
    assert_eq!(harness.phase(), None);
}

#[tokio::test]
async fn a_failed_install_starts_codex_again_and_leaves_its_chooser_to_a_person() {
    let mut harness = codex_at_update_chooser();
    let now = Instant::now();
    assert!(harness.app.supervise_self_updates(now));
    assert_eq!(
        harness.writes.try_recv().unwrap(),
        Bytes::from_static(b"\r")
    );

    harness.exit_with(
        "Updating Codex via `npm install -g @openai/codex`...\r\n\
         npm error code EACCES\r\n\
         Error: `npm install -g @openai/codex` failed with status exit status: 243",
    );
    assert!(harness.app.supervise_self_updates(now));
    assert!(String::from_utf8_lossy(&harness.writes.try_recv().unwrap()).contains("codex"));
    assert_eq!(harness.terminal().agent_name.as_deref(), Some(NAME));
    let error = harness.update_error().unwrap();
    assert!(
        error.contains("failed with status exit status: 243"),
        "{error}"
    );
    assert!(error.contains("Answer the update prompt"), "{error}");

    // The relaunched Codex offers the update again: Bus does not answer.
    harness.show(CHOOSER);
    harness
        .terminal()
        .set_detected_state(Some(AgentKind::Codex), AgentState::Blocked);
    assert!(!harness.app.supervise_self_updates(now));
    assert!(harness.writes.try_recv().is_err());
    assert_eq!(harness.update_error(), Some(error));
}

#[tokio::test]
async fn an_update_offered_again_after_relaunch_is_left_to_a_person() {
    let mut harness = codex_at_update_chooser();
    let now = Instant::now();
    assert!(harness.app.supervise_self_updates(now));
    harness.writes.try_recv().unwrap();
    harness.exit_with("🎉 Update ran successfully! Please restart Codex.");
    assert!(harness.app.supervise_self_updates(now));
    harness.writes.try_recv().unwrap();

    harness.show(CHOOSER);
    harness
        .terminal()
        .set_detected_state(Some(AgentKind::Codex), AgentState::Blocked);
    assert!(harness.app.supervise_self_updates(now));
    assert!(harness.writes.try_recv().is_err());
    assert!(harness
        .update_error()
        .is_some_and(|error| error.contains("offered the same update again")));
}

#[tokio::test]
async fn a_hung_install_is_interrupted_then_reported_as_failed() {
    let mut harness = codex_at_update_chooser();
    let now = Instant::now();
    assert!(harness.app.supervise_self_updates(now));
    harness.writes.try_recv().unwrap();
    assert_eq!(
        harness.app.state.next_self_update_deadline(),
        Some(now + SELF_UPDATE_INSTALL_TIMEOUT)
    );

    let late = now + SELF_UPDATE_INSTALL_TIMEOUT;
    assert!(harness.app.supervise_self_updates(late));
    assert_eq!(
        harness.writes.try_recv().unwrap(),
        Bytes::from_static(b"\x03")
    );
    assert!(matches!(
        harness.phase(),
        Some(SelfUpdatePhase::Installing {
            interrupted: true,
            ..
        })
    ));

    harness.exit_with("^C");
    assert!(harness.app.supervise_self_updates(late));
    assert!(String::from_utf8_lossy(&harness.writes.try_recv().unwrap()).contains("codex"));
    assert!(harness
        .update_error()
        .is_some_and(|error| error.contains("did not finish within 5 minutes")));
}

#[tokio::test]
async fn an_agent_bus_did_not_start_keeps_its_chooser() {
    let mut harness = codex_at_update_chooser();
    harness.terminal().managed_agent_args = None;
    assert!(!harness.app.supervise_self_updates(Instant::now()));
    assert!(harness.writes.try_recv().is_err());
    assert_eq!(harness.phase(), None);
}
