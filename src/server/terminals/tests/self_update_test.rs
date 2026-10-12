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

#[test]
fn a_restored_agent_relaunches_with_the_command_its_restore_typed() {
    let resume: Vec<String> = ["codex", "resume", "saved-session", "-c", "hooks=bus"]
        .map(String::from)
        .into();
    assert_eq!(
        managed_relaunch_args(Some(AgentKind::Codex), &resume),
        Some(resume[1..].to_vec())
    );
    // Not the managed agent's own executable, or no managed agent: no relaunch.
    assert_eq!(
        managed_relaunch_args(Some(AgentKind::Claude), &resume),
        None
    );
    assert_eq!(managed_relaunch_args(None, &resume), None);
    assert_eq!(managed_relaunch_args(Some(AgentKind::Codex), &[]), None);
}

#[tokio::test]
async fn a_restored_agent_resumes_its_own_session_after_the_update() {
    let mut harness = codex_at_update_chooser();
    // As after a Bus restart: restored, not started, with the resume command.
    let terminal = harness.terminal();
    terminal.clear_agent_name();
    terminal.restore_managed_agent(NAME.into(), AgentKind::Codex);
    terminal.set_persisted_agent_session(crate::agents::resume::catalog::PersistedAgentSession {
        source: "herdr:codex".into(),
        agent: "codex".into(),
        session_ref: crate::agents::resume::catalog::AgentSessionRef::id("saved-session").unwrap(),
    });
    terminal.managed_agent_args = Some(
        ["resume", "saved-session", "-c", "hooks=bus"]
            .map(String::from)
            .into(),
    );
    terminal.set_detected_state(Some(AgentKind::Codex), AgentState::Blocked);
    let now = Instant::now();
    assert!(harness.app.supervise_self_updates(now));
    assert_eq!(
        harness.writes.try_recv().unwrap(),
        Bytes::from_static(b"\r")
    );

    harness.exit_with("🎉 Update ran successfully! Please restart Codex.");
    assert!(harness.app.supervise_self_updates(now));
    let relaunch = String::from_utf8_lossy(&harness.writes.try_recv().unwrap()).into_owned();
    assert!(
        relaunch.contains("codex resume saved-session"),
        "{relaunch}"
    );
    let terminal = harness.terminal();
    assert_eq!(terminal.agent_name.as_deref(), Some(NAME));
    assert_eq!(
        terminal
            .persisted_agent_session
            .as_ref()
            .map(|session| session.session_ref.value.as_str()),
        Some("saved-session")
    );
}

#[tokio::test]
async fn claude_update_notices_never_block_or_open_a_dialog() {
    // Claude Code updates itself in the background and reports it in the
    // footer only; the session keeps working on the old version until its
    // next launch. Captured live from a scratch 2.1.295 native install that
    // updated itself to 2.1.296.
    let capture =
        include_bytes!("../../../../tests/fixtures/claude-update/update-installed-2.1.295.ansi");
    let runtime = crate::terminal::TerminalRuntime::test_with_screen_bytes(120, 40, capture);
    let installed = runtime.visible_text();
    let ansi = runtime.visible_ansi();
    assert!(
        installed.contains("Update installed · Restart to update"),
        "{installed}"
    );
    // The other footers of the 2.1.296 updater, verbatim from its bundle.
    let failed = installed.replace(
        "✔ Update installed · Restart to update",
        "✘ Auto-update failed · Run claude doctor",
    );
    let package_manager = installed.replace(
        "✔ Update installed · Restart to update",
        "Update available! Run: npm i -g @anthropic-ai/claude-code (auto-update failed)",
    );
    for screen in [&installed, &failed, &package_manager] {
        let result = crate::agents::manifest::detect_with_osc(
            AgentKind::Claude,
            crate::agents::manifest::DetectionInput {
                screen,
                osc_title: "",
                osc_progress: "",
            },
        );
        assert_ne!(result.state, AgentState::Blocked, "{screen}");
        assert!(!result.visible_blocker, "{screen}");
        assert!(crate::agents::manifest::auto_update(AgentKind::Claude, screen).is_none());
    }
    assert!(crate::agents::dialog::parse(&ansi).is_none());
    runtime.shutdown();
}

#[tokio::test]
async fn a_bus_started_codex_installing_its_update_is_listed_working_and_ready() {
    let mut harness = codex_at_update_chooser();
    let now = Instant::now();
    // As live: the chooser moves the pending launch to its Blocked phase.
    harness.terminal().reconcile_managed_agent_at(now, false);
    assert!(harness.app.supervise_self_updates(now));
    harness.writes.try_recv().unwrap();

    harness.show(b"\x1b[2J\x1b[HUpdating Codex via `npm install -g @openai/codex`...\r\n");
    harness
        .terminal()
        .set_detected_state(Some(AgentKind::Codex), AgentState::Working);
    harness.terminal().reconcile_managed_agent_at(now, false);
    let info = harness.app.agent_info(0, harness.pane).unwrap();
    assert_eq!(info.agent_status, schema::AgentStatus::Working);
    assert_eq!(info.dialog_id, None);
    // The coordinator reads launch_pending before agent_status: a pending
    // launch would read Launching, and its queued messages would stall.
    assert!(!info.launch_pending);
    assert!(info.interactive_ready);
}

#[tokio::test]
async fn a_codex_relaunched_after_its_update_is_listed_ready_under_the_same_identity() {
    let mut harness = codex_at_update_chooser();
    let before = harness.app.agent_info(0, harness.pane).unwrap();
    let now = Instant::now();
    assert!(harness.app.supervise_self_updates(now));
    harness.writes.try_recv().unwrap();
    harness.exit_with("🎉 Update ran successfully! Please restart Codex.");
    // Between the exit and the relaunch the pane is not an agent.
    assert!(harness.app.agent_info(0, harness.pane).is_none());
    assert!(harness.app.supervise_self_updates(now));
    harness.writes.try_recv().unwrap();

    harness.show(b"\x1b[2J\x1b[H\xe2\x80\xba Ask Codex to do anything\r\n");
    harness
        .terminal()
        .set_detected_state(Some(AgentKind::Codex), AgentState::Idle);
    harness
        .terminal()
        .reconcile_managed_agent_at(now + std::time::Duration::from_secs(5), false);
    let info = harness.app.agent_info(0, harness.pane).unwrap();
    // What agent.prompt_if_unbound checks before typing a queued message.
    assert_eq!(info.terminal_id, before.terminal_id);
    assert_eq!(info.pane_id, before.pane_id);
    assert_eq!(info.name.as_deref(), Some(NAME));
    assert_eq!(info.agent.as_deref(), Some("codex"));
    assert_eq!(info.agent_session, None);
    assert_eq!(info.agent_status, schema::AgentStatus::Idle);
    assert_eq!(info.dialog_id, None);
    assert!(!info.launch_pending && info.interactive_ready);
}

#[tokio::test]
async fn a_restored_codex_relaunched_after_its_update_is_listed_with_its_saved_session_once_ready()
{
    let mut harness = codex_at_update_chooser();
    let terminal = harness.terminal();
    terminal.clear_agent_name();
    terminal.restore_managed_agent(NAME.into(), AgentKind::Codex);
    terminal.set_persisted_agent_session(crate::agents::resume::catalog::PersistedAgentSession {
        source: "herdr:codex".into(),
        agent: "codex".into(),
        session_ref: crate::agents::resume::catalog::AgentSessionRef::id("saved-session").unwrap(),
    });
    terminal.managed_agent_args = Some(
        ["resume", "saved-session", "-c", "hooks=bus"]
            .map(String::from)
            .into(),
    );
    terminal.set_detected_state(Some(AgentKind::Codex), AgentState::Blocked);
    let now = Instant::now();
    assert!(harness.app.supervise_self_updates(now));
    harness.writes.try_recv().unwrap();
    harness.exit_with("🎉 Update ran successfully! Please restart Codex.");
    assert!(harness.app.supervise_self_updates(now));
    harness.writes.try_recv().unwrap();

    harness.show(b"\x1b[2J\x1b[H\xe2\x80\xba Ask Codex to do anything\r\n");
    harness
        .terminal()
        .set_detected_state(Some(AgentKind::Codex), AgentState::Idle);
    harness
        .terminal()
        .reconcile_managed_agent_at(now + std::time::Duration::from_secs(5), false);
    let info = harness.app.agent_info(0, harness.pane).unwrap();
    // What agent.prompt_if_idle checks against the coordinator's session.
    assert_eq!(
        info.agent_session
            .as_ref()
            .map(|session| session.value.as_str()),
        Some("saved-session")
    );
    assert_eq!(info.agent_status, schema::AgentStatus::Idle);
    assert!(!info.launch_pending && info.interactive_ready);
}

#[tokio::test]
async fn a_relaunch_that_fails_past_its_deadline_drops_the_update() {
    let mut harness = codex_at_update_chooser();
    let now = Instant::now();
    assert!(harness.app.supervise_self_updates(now));
    harness.writes.try_recv().unwrap();
    harness.exit_with("🎉 Update ran successfully! Please restart Codex.");
    // The pane no longer takes input, so every relaunch attempt fails.
    let (_, closed) = tokio::sync::mpsc::channel::<Bytes>(1);
    harness.writes = closed;
    harness.app.supervise_self_updates(now);
    let late = now + SELF_UPDATE_EXIT_TIMEOUT + std::time::Duration::from_secs(1);
    harness.app.supervise_self_updates(late);
    // The pane is a plain shell: nothing can list an error for it, and a
    // stale one must not show if a person starts the agent there by hand.
    assert_eq!(harness.phase(), None);
    assert!(harness.app.agent_info(0, harness.pane).is_none());
}

#[tokio::test]
async fn an_older_codex_update_chooser_is_answered_too() {
    let mut harness = codex_at_update_chooser();
    let chooser = "\x1b[2J\x1b[H\u{2728} Update available! 0.153.0 -> 9.8.7\r\n\r\n\
        Release notes: https://example\r\n\r\n\
        \u{203a} 1. Update now (runs `npm install -g @openai/codex`)\r\n\
        \x20 2. Skip\r\n\
        \x20 3. Skip until next version\r\n\r\n\
        Press enter to continue";
    harness.show(chooser.as_bytes());
    harness
        .terminal()
        .set_detected_state(Some(AgentKind::Codex), AgentState::Blocked);
    assert!(crate::agents::manifest::auto_update(
        AgentKind::Codex,
        &harness
            .app
            .terminal_runtimes
            .get(&harness.terminal_id)
            .unwrap()
            .visible_text()
    )
    .is_some());
    assert!(harness.app.supervise_self_updates(Instant::now()));
    assert!(harness.writes.try_recv().is_ok());
}

#[tokio::test]
async fn updater_exiting_after_interrupt_grace_is_relaunched_for_manual_recovery() {
    let mut harness = codex_at_update_chooser();
    let now = Instant::now();
    harness.terminal().reconcile_managed_agent_at(now, false);
    assert!(harness.app.supervise_self_updates(now));
    harness.writes.try_recv().unwrap();
    harness.show(b"\x1b[2J\x1b[HUpdating Codex via `npm install -g @openai/codex`...\r\n");
    harness
        .terminal()
        .set_detected_state(Some(AgentKind::Codex), AgentState::Working);
    let late = now + SELF_UPDATE_INSTALL_TIMEOUT;
    assert!(harness.app.supervise_self_updates(late));
    assert_eq!(
        harness.writes.try_recv().unwrap(),
        Bytes::from_static(b"\x03")
    );
    assert!(harness
        .app
        .supervise_self_updates(late + SELF_UPDATE_EXIT_TIMEOUT));
    // The updater is still on screen, so the error asks to stop it, not to
    // answer a chooser that is not there.
    let info = harness.app.agent_info(0, harness.pane).unwrap();
    assert_eq!(info.dialog_id, None);
    assert!(
        info.update_error
            .as_deref()
            .is_some_and(|error| error.contains("Stop it in")),
        "{:?}",
        info.update_error
    );
    // The updater eventually handles Ctrl-C, after Bus's exit grace elapsed.
    harness.exit_with("^C");
    harness.app.supervise_self_updates(
        late + SELF_UPDATE_EXIT_TIMEOUT + std::time::Duration::from_secs(1),
    );
    let relaunch = harness.writes.try_recv().ok();
    assert!(
        relaunch.is_some(),
        "late updater exit never relaunched Codex; phase={:?}",
        harness.phase()
    );
    assert!(String::from_utf8_lossy(&relaunch.unwrap()).contains("codex"));
}
