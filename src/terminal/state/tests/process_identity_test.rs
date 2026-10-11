// Session reports carry the reporting hook's process chain, trimmed below the pane
// shell. The detector names the agent's job leader. These tests pin the C15 fence:
// a retired leader's reports never bind, while a live or new process still does.

fn instance(pid: u32) -> crate::platform::ProcessInstance {
    crate::platform::ProcessInstance {
        pid,
        birth: u64::from(pid) * 10,
    }
}

fn chain(pids: &[u32]) -> Vec<crate::platform::ProcessInstance> {
    pids.iter().copied().map(instance).collect()
}

fn report(
    terminal: &mut TerminalState,
    agent: &str,
    session: &str,
    seq: u64,
    source: &str,
    reporter: &[u32],
) -> Option<TerminalStateMutation> {
    terminal.set_agent_session_ref_for_session_start(
        format!("herdr:{agent}"),
        agent.into(),
        Some(session_id(session)),
        Some(seq),
        Some(source.into()),
        &chain(reporter),
    )
}

fn saved(terminal: &TerminalState) -> Option<String> {
    terminal
        .persisted_agent_session
        .as_ref()
        .map(|session| session.session_ref.value.clone())
}

fn detect(terminal: &mut TerminalState, agent: AgentKind, replaced: Option<u32>) {
    terminal.set_detected_agent_process_at(agent, replaced.map(instance), Instant::now());
}

fn exit(terminal: &mut TerminalState, agent: AgentKind, leader: Option<u32>) {
    terminal.set_detected_state_with_screen_signals_at(
        Some(agent),
        AgentState::Idle,
        false,
        true,
        leader.map(instance),
        Instant::now(),
    );
}

#[test]
fn delayed_session_report_does_not_restore_an_exited_agent_session() {
    let mut terminal = test_terminal();
    detect(&mut terminal, AgentKind::Codex, None);
    report(&mut terminal, "codex", "exited-session", 1, "startup", &[101, 100]).unwrap();
    exit(&mut terminal, AgentKind::Codex, Some(100));
    assert!(terminal.persisted_agent_session.is_none());

    // A later callback from the same process can be consumed after its exit.
    report(&mut terminal, "codex", "exited-session", 2, "compact", &[102, 100]);

    assert!(
        terminal.persisted_agent_session.is_none(),
        "a delayed callback must not make the exited session resumable again"
    );
    assert!(terminal.effective_agent_label().is_none());
}

#[test]
fn late_old_report_after_replacement_does_not_replace_the_new_codex_session() {
    let mut terminal = test_terminal();
    detect(&mut terminal, AgentKind::Codex, None);
    report(&mut terminal, "codex", "old", 1, "startup", &[101, 100]).unwrap();
    exit(&mut terminal, AgentKind::Codex, Some(100));
    detect(&mut terminal, AgentKind::Codex, None);
    report(&mut terminal, "codex", "new", 3, "startup", &[201, 200]).unwrap();

    // Codex lets `compact` replace a session, so only the process fence stops this.
    assert!(report(&mut terminal, "codex", "old", 4, "compact", &[102, 100]).is_none());
    assert_eq!(saved(&terminal).as_deref(), Some("new"));
}

#[test]
fn late_old_report_before_the_new_claude_report_does_not_block_it() {
    let mut terminal = test_terminal();
    detect(&mut terminal, AgentKind::Claude, None);
    report(&mut terminal, "claude", "old", 1, "startup", &[101, 100]).unwrap();
    exit(&mut terminal, AgentKind::Claude, Some(100));

    assert!(report(&mut terminal, "claude", "old", 2, "compact", &[102, 100]).is_none());
    report(&mut terminal, "claude", "new", 3, "startup", &[201, 200]).unwrap();
    assert_eq!(saved(&terminal).as_deref(), Some("new"));
}

#[test]
fn a_same_kind_replacement_without_an_observed_exit_retires_the_old_process() {
    let mut terminal = test_terminal();
    detect(&mut terminal, AgentKind::Codex, None);
    report(&mut terminal, "codex", "old", 1, "startup", &[101, 100]).unwrap();
    detect(&mut terminal, AgentKind::Codex, Some(100));
    assert!(
        terminal.persisted_agent_session.is_none(),
        "the replaced process's session is no longer resumable"
    );

    assert!(report(&mut terminal, "codex", "old", 2, "compact", &[103, 100]).is_none());
    report(&mut terminal, "codex", "new", 3, "startup", &[201, 200]).unwrap();
    assert_eq!(saved(&terminal).as_deref(), Some("new"));
}

#[test]
fn a_fast_restart_report_inside_the_exit_window_is_accepted() {
    let mut terminal = test_terminal();
    detect(&mut terminal, AgentKind::Codex, None);
    report(&mut terminal, "codex", "old", 1, "startup", &[101, 100]).unwrap();
    exit(&mut terminal, AgentKind::Codex, Some(100));

    // The detector has not seen the new process yet; its exit marker is still set.
    report(&mut terminal, "codex", "old", 2, "resume", &[201, 200]).unwrap();
    assert_eq!(saved(&terminal).as_deref(), Some("old"));
}

#[test]
fn clear_and_new_in_the_same_live_process_still_replace_the_session() {
    for (agent, kind, first, second) in [
        ("claude", AgentKind::Claude, "startup", "clear"),
        ("codex", AgentKind::Codex, "startup", "clear"),
        ("cursor", AgentKind::Cursor, "new", "new"),
    ] {
        let mut terminal = test_terminal();
        detect(&mut terminal, kind, None);
        exit(&mut terminal, kind, Some(50));
        detect(&mut terminal, kind, None);
        report(&mut terminal, agent, "a", 1, first, &[101, 100]).unwrap();
        report(&mut terminal, agent, "b", 2, second, &[102, 100]).unwrap();
        assert_eq!(saved(&terminal).as_deref(), Some("b"), "{agent}");
    }
}

#[test]
fn a_rejected_report_does_not_consume_the_sequence() {
    let mut terminal = test_terminal();
    detect(&mut terminal, AgentKind::Codex, None);
    exit(&mut terminal, AgentKind::Codex, Some(100));
    assert!(report(&mut terminal, "codex", "old", 10, "compact", &[101, 100]).is_none());
    report(&mut terminal, "codex", "new", 6, "startup", &[201, 200]).unwrap();
    assert_eq!(saved(&terminal).as_deref(), Some("new"));
}

#[test]
fn the_first_report_of_a_new_process_is_a_fresh_binding() {
    let mut terminal = test_terminal();
    detect(&mut terminal, AgentKind::Claude, None);
    report(&mut terminal, "claude", "a", 1, "startup", &[101, 100]).unwrap();

    // Claude `startup` cannot replace a session reported by the same process,
    // but this report shares no process with the one that bound `a`.
    report(&mut terminal, "claude", "b", 2, "startup", &[201, 200]).unwrap();
    assert_eq!(saved(&terminal).as_deref(), Some("b"));
}

#[test]
fn an_old_exit_does_not_clear_a_session_a_newer_process_reported() {
    let mut terminal = test_terminal();
    detect(&mut terminal, AgentKind::Codex, None);
    report(&mut terminal, "codex", "a", 1, "startup", &[101, 100]).unwrap();
    report(&mut terminal, "codex", "b", 2, "startup", &[201, 200]).unwrap();
    exit(&mut terminal, AgentKind::Codex, Some(100));
    assert_eq!(saved(&terminal).as_deref(), Some("b"));
}

#[test]
fn a_report_without_a_process_chain_follows_the_existing_rules() {
    // Hooks spooled by an older Bus carry no chain; they keep today's behavior.
    let mut terminal = test_terminal();
    detect(&mut terminal, AgentKind::Codex, None);
    report(&mut terminal, "codex", "a", 1, "startup", &[101, 100]).unwrap();
    exit(&mut terminal, AgentKind::Codex, Some(100));
    report(&mut terminal, "codex", "a", 2, "compact", &[]).unwrap();
    assert_eq!(saved(&terminal).as_deref(), Some("a"));
}

#[test]
fn a_reused_pid_with_a_new_birth_is_not_retired() {
    let mut terminal = test_terminal();
    detect(&mut terminal, AgentKind::Codex, None);
    exit(&mut terminal, AgentKind::Codex, Some(100));
    let reused = crate::platform::ProcessInstance { pid: 100, birth: 7 };
    terminal
        .set_agent_session_ref_for_session_start(
            "herdr:codex".into(),
            "codex".into(),
            Some(session_id("new")),
            Some(1),
            Some("startup".into()),
            &[instance(101), reused],
        )
        .unwrap();
    assert_eq!(saved(&terminal).as_deref(), Some("new"));
}
