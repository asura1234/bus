// Included by dialogs_test.rs to exercise notify_dialogs with its ordinary
// no-input transport fixture. These are semantic native-API observations, not
// raw provider captures. The terminal driver's separate producer tests pin this
// opaque content ID from the captured title/body/options across wraps and hints.
const LATENCY_EPISODE: &str = "33415108d1173ade22a8d5b1cf5b92e1e687caa413866bc2071184578a2b8b14";
const KEEP_WAITING: &str = "Dismiss and keep waiting";

struct AutomaticScratch(PathBuf);

impl Drop for AutomaticScratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn latency_screen(provider: Provider) -> String {
    if provider == Provider::Codex {
        let captured: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/provider-ui/codex-extra-thought.json"
        ))
        .unwrap();
        let options = captured["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|option| {
                format!(
                    "{} {}. {}",
                    if option["selected"] == true {
                        "❯"
                    } else {
                        " "
                    },
                    option["number"],
                    option["label"].as_str().unwrap(),
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        // The footer is replay scaffolding, as documented by the fixture README.
        format!(
            "{}\n\n{options}\n\nEnter to select · Esc to cancel",
            captured["text"].as_str().unwrap()
        )
    } else {
        include_str!("../../../../tests/fixtures/provider-ui/claude-extra-thought.txt").into()
    }
}

fn configure_latency(
    screen: &mut Screen,
    dir: &std::path::Path,
    agent: AgentId,
    provider: Provider,
) {
    screen.dialog = true;
    screen.episode_override = Some(LATENCY_EPISODE.into());
    screen.native_provider = Some(provider.label().into());
    screen.raw_screen = Some(latency_screen(provider));
    screen.allow_driver_input = true;
    screen.durable_reservation = Some((dir.join("state.json"), agent));
}

/// Build wire cells directly: the messaging fixture must not depend on terminal
/// parsing or use the production blocker as its own expected-ID oracle.
fn automatic_observation(screen: &Screen) -> schema::TerminalObservation {
    let text = screen.raw_screen.as_deref().unwrap();
    let text = if screen.selected_option == 1 {
        text.replace("❯ 1.", "  1.").replace("  2.", "❯ 2.")
    } else {
        text.to_owned()
    };
    let blank = schema::driver::Cell {
        symbol: " ".into(),
        fg: "default".into(),
        bg: "default".into(),
        attrs: Vec::new(),
        link: None,
    };
    let mut cells = vec![vec![blank; 120]; 12];
    for (row, line) in text.lines().enumerate() {
        for (col, ch) in line.chars().enumerate() {
            cells[row][col].symbol = ch.to_string();
        }
    }
    schema::TerminalObservation {
        terminal_id: "terminal".into(),
        pane_id: "pane".into(),
        content_revision: 2,
        text,
        grid: schema::driver::Grid {
            cols: 120,
            rows: 12,
            cells,
            cursor: None,
        },
        focus: Some((
            3 + screen.selected_option,
            if screen.selected_option == 0 {
                "Retry with a faster model".into()
            } else {
                KEEP_WAITING.into()
            },
        )),
        focus_occurrence: 0,
    }
}

fn automatic_press(
    screen: &mut Screen,
    params: schema::AgentPressParams,
) -> Result<ResponseResult, TransportError> {
    let (path, agent) = screen.durable_reservation.as_ref().unwrap();
    let durable = crate::messaging::storage::state_store::JsonStore::new(path.clone())
        .load()
        .unwrap()
        .unwrap();
    assert!(
        durable
            .agent(*agent)
            .unwrap()
            .automatic_dialog_answer
            .is_some(),
        "the episode reservation must be on disk before the first native input"
    );
    screen.driver_presses.push(params);
    if screen.press_uncertain {
        return Err(TransportError {
            code: None,
            message: "input acknowledgement lost".into(),
            definitely_rejected: false,
        });
    }
    let observation = automatic_observation(screen);
    let selection_pending = screen.selected_option == 0;
    if selection_pending && !screen.press_stalled {
        screen.selected_option = 1;
    }
    if !selection_pending && screen.clear_on_confirm {
        screen.dialog = false;
    }
    Ok(ResponseResult::TerminalPressed {
        press: schema::TerminalPressResult {
            keys: vec![if selection_pending { "down" } else { "enter" }.into()],
            text_written: false,
            selection_pending,
            observation,
        },
    })
}

#[test]
fn latency_notice_selects_keep_waiting_for_claude_and_codex_without_paging() {
    for provider in [Provider::ClaudeCode, Provider::Codex] {
        let (mut worker, agent, room, _, screen, dir) = worker_with_provider(true, provider);
        let _scratch = AutomaticScratch(dir.clone());
        configure_latency(&mut screen.lock().unwrap(), &dir, agent, provider);
        notify_steady(&mut worker, agent, Some(LATENCY_EPISODE));
        let native = screen.lock().unwrap();
        assert_eq!(native.driver_presses.len(), 2, "{provider:?}");
        for press in &native.driver_presses {
            assert_eq!(press.target, "pane");
            assert_eq!(press.expected_terminal_id.as_deref(), Some("terminal"));
            assert_eq!(press.select.as_deref(), Some(KEEP_WAITING));
            assert!(
                press.keys.is_empty(),
                "policy must select a label, never a number"
            );
            assert!(press.text.is_none());
            assert!(!press.submit);
        }
        assert_eq!(
            native.driver_observations, 1,
            "use the redraw-aware press route"
        );
        assert_eq!(
            native.native_gets, 1,
            "reuse the existing current-blocker check"
        );
        assert!(sent_by(&worker, room, agent).is_empty());
        assert!(worker
            .state
            .agent(agent)
            .unwrap()
            .automatic_dialog_answer
            .is_some());
    }
}

#[test]
fn latency_notice_with_same_content_for_other_providers_still_needs_help() {
    for provider in [Provider::Cursor, Provider::Grok, Provider::Kimi] {
        let (mut worker, agent, room, _, screen, dir) = worker_with_provider(true, provider);
        let _scratch = AutomaticScratch(dir.clone());
        configure_latency(&mut screen.lock().unwrap(), &dir, agent, provider);
        screen.lock().unwrap().allow_driver_input = false;
        notify_steady(&mut worker, agent, Some(LATENCY_EPISODE));
        notify_steady(&mut worker, agent, Some(LATENCY_EPISODE));
        assert_eq!(screen.lock().unwrap().inputs, 0, "{provider:?}");
        assert_eq!(sent_by(&worker, room, agent).len(), 1, "{provider:?}");
        assert!(worker
            .state
            .agent(agent)
            .unwrap()
            .automatic_dialog_answer
            .is_none());
    }
}

#[test]
fn latency_policy_excludes_permission_update_and_changed_menu_content() {
    for (episode, text) in [
        (
            "command-approval",
            "Run this command?\n❯ 1. Approve once\n  2. Deny\nEnter select · Esc cancel",
        ),
        (
            "file-permission",
            "Write this file?\n❯ 1. Allow\n  2. Deny\nEnter select · Esc cancel",
        ),
        (
            "update-offer",
            "Update available\n❯ 1. Install\n  2. Later\nEnter select · Esc cancel",
        ),
        (
            "changed-latency-title",
            "A different request needs attention",
        ),
        (
            "changed-latency-body",
            "Retry will change the existing model",
        ),
        ("changed-latency-option", "Dismiss and change model"),
        (
            "6b07b9840a8029b43e2421646d6f010691ff6f9ddd5dce4d180b4cfba5edc4a7",
            "former physical-row replay ID",
        ),
    ] {
        let (mut worker, agent, room, _, screen, dir) = worker(true);
        let _scratch = AutomaticScratch(dir);
        {
            let mut native = screen.lock().unwrap();
            native.dialog = true;
            native.episode_override = Some(episode.into());
            native.raw_screen = Some(text.into());
        }
        notify_steady(&mut worker, agent, Some(episode));
        assert_eq!(screen.lock().unwrap().inputs, 0, "{episode}");
        assert_eq!(sent_by(&worker, room, agent).len(), 1, "{episode}");
        assert!(worker
            .state
            .agent(agent)
            .unwrap()
            .automatic_dialog_answer
            .is_none());
    }
}

#[test]
fn latency_policy_refuses_unreadable_and_replaced_native_identity() {
    for boundary in ["redraw", "terminal", "pane", "session"] {
        let (mut worker, agent, room, _, screen, dir) = worker(true);
        let _scratch = AutomaticScratch(dir.clone());
        configure_latency(
            &mut screen.lock().unwrap(),
            &dir,
            agent,
            Provider::ClaudeCode,
        );
        let mut state = worker.state.clone();
        let mut identity = state.agent(agent).unwrap().runtime_identity.clone();
        identity.session_id = Some("original-session".into());
        state.set_agent_runtime_identity(agent, identity).unwrap();
        worker.save(state).unwrap();
        {
            let mut native = screen.lock().unwrap();
            native.session = Some("original-session".into());
            native.allow_driver_input = false;
            match boundary {
                "redraw" => native.unavailable_reads = u32::MAX,
                "terminal" => native.wrong_terminal = true,
                "pane" => native.wrong_pane = true,
                "session" => native.session = Some("replacement-session".into()),
                _ => unreachable!(),
            }
        }
        notify_steady(&mut worker, agent, Some(LATENCY_EPISODE));
        assert_eq!(screen.lock().unwrap().inputs, 0, "{boundary}");
        assert!(sent_by(&worker, room, agent).is_empty(), "{boundary}");
        assert!(worker
            .state
            .agent(agent)
            .unwrap()
            .automatic_dialog_answer
            .is_none());
    }
}

#[test]
fn latency_attempt_is_not_replayed_after_uncertain_or_stalled_input() {
    for stalled in [false, true] {
        let (mut worker, agent, room, _, screen, dir) = worker(true);
        let _scratch = AutomaticScratch(dir.clone());
        {
            let mut native = screen.lock().unwrap();
            configure_latency(&mut native, &dir, agent, Provider::ClaudeCode);
            native.press_uncertain = !stalled;
            native.press_stalled = stalled;
        }
        notify_steady(&mut worker, agent, Some(LATENCY_EPISODE));
        assert_eq!(screen.lock().unwrap().inputs, 1);
        assert_eq!(sent_by(&worker, room, agent).len(), 1);
        for _ in 0..3 {
            notify_steady(&mut worker, agent, Some(LATENCY_EPISODE));
        }
        assert_eq!(
            screen.lock().unwrap().inputs,
            1,
            "spent attempt must not replay"
        );
        assert_eq!(
            sent_by(&worker, room, agent).len(),
            1,
            "help is once per episode"
        );
        assert!(worker
            .store
            .load()
            .unwrap()
            .unwrap()
            .agent(agent)
            .unwrap()
            .automatic_dialog_answer
            .is_some());
    }
}

#[test]
fn selected_latency_menu_that_remains_visible_gets_help_without_second_attempt() {
    let (mut worker, agent, room, _, screen, dir) = worker(true);
    let _scratch = AutomaticScratch(dir.clone());
    configure_latency(
        &mut screen.lock().unwrap(),
        &dir,
        agent,
        Provider::ClaudeCode,
    );
    notify_steady(&mut worker, agent, Some(LATENCY_EPISODE));
    assert_eq!(screen.lock().unwrap().inputs, 2);
    assert!(sent_by(&worker, room, agent).is_empty());
    notify_steady(&mut worker, agent, Some(LATENCY_EPISODE));
    notify_steady(&mut worker, agent, Some(LATENCY_EPISODE));
    assert_eq!(screen.lock().unwrap().inputs, 2);
    assert_eq!(sent_by(&worker, room, agent).len(), 1);
}

#[test]
fn latency_attempt_reservation_survives_coordinator_restart() {
    let (mut worker, agent, room, _, screen, dir) = worker(true);
    let _scratch = AutomaticScratch(dir.clone());
    {
        let mut native = screen.lock().unwrap();
        configure_latency(&mut native, &dir, agent, Provider::ClaudeCode);
        native.press_uncertain = true;
    }
    notify_steady(&mut worker, agent, Some(LATENCY_EPISODE));
    assert_eq!(screen.lock().unwrap().inputs, 1);
    drop(worker);
    let mut restored = Worker::open(dir, Box::new(FakeServer(screen.clone()))).unwrap();
    notify_steady(&mut restored, agent, Some(LATENCY_EPISODE));
    notify_steady(&mut restored, agent, Some(LATENCY_EPISODE));
    assert_eq!(screen.lock().unwrap().inputs, 1);
    assert_eq!(sent_by(&restored, room, agent).len(), 1);
    assert!(restored
        .state
        .agent(agent)
        .unwrap()
        .automatic_dialog_answer
        .is_some());
}

#[test]
fn a_clear_latency_episode_without_notice_releases_reservation_for_next_menu() {
    let (mut worker, agent, room, _, screen, dir) = worker(true);
    let _scratch = AutomaticScratch(dir.clone());
    {
        let mut native = screen.lock().unwrap();
        configure_latency(&mut native, &dir, agent, Provider::ClaudeCode);
        native.clear_on_confirm = true;
    }
    notify_steady(&mut worker, agent, Some(LATENCY_EPISODE));
    assert_eq!(screen.lock().unwrap().inputs, 2);
    assert!(worker.state.agent(agent).unwrap().dialog_notice.is_none());
    notify_steady(&mut worker, agent, None);
    assert!(worker
        .state
        .agent(agent)
        .unwrap()
        .automatic_dialog_answer
        .is_none());
    assert!(worker
        .store
        .load()
        .unwrap()
        .unwrap()
        .agent(agent)
        .unwrap()
        .automatic_dialog_answer
        .is_none());
    {
        let mut native = screen.lock().unwrap();
        native.dialog = true;
        native.selected_option = 0;
    }
    notify_steady(&mut worker, agent, Some(LATENCY_EPISODE));
    assert_eq!(screen.lock().unwrap().inputs, 4);
    assert!(sent_by(&worker, room, agent).is_empty());
}

#[test]
fn a_new_managed_launch_can_attempt_the_same_latency_menu_once() {
    let (mut worker, agent, room, _, screen, dir) = worker(true);
    let _scratch = AutomaticScratch(dir.clone());
    configure_latency(
        &mut screen.lock().unwrap(),
        &dir,
        agent,
        Provider::ClaudeCode,
    );
    notify_steady(&mut worker, agent, Some(LATENCY_EPISODE));
    assert_eq!(screen.lock().unwrap().inputs, 2);
    let old = worker
        .state
        .agent(agent)
        .unwrap()
        .automatic_dialog_answer
        .clone();
    let mut state = worker.state.clone();
    let mut identity = state.agent(agent).unwrap().runtime_identity.clone();
    identity.launch_id = Some("next-launch".into());
    state.set_agent_runtime_identity(agent, identity).unwrap();
    worker.save(state).unwrap();
    screen.lock().unwrap().selected_option = 0;
    // This unchanged screen is already steady. Inspect the fresh launch's
    // selection poll; a later poll of the still-visible menu should seek help.
    worker
        .notify_dialogs(vec![(agent, Some(LATENCY_EPISODE.into()))])
        .unwrap();
    assert_eq!(screen.lock().unwrap().inputs, 4);
    assert_ne!(
        worker.state.agent(agent).unwrap().automatic_dialog_answer,
        old
    );
    assert!(sent_by(&worker, room, agent).is_empty());
}

#[test]
fn a_failed_latency_reservation_save_sends_no_input() {
    let (mut worker, agent, room, _, screen, dir) = worker(true);
    let _scratch = AutomaticScratch(dir.clone());
    configure_latency(
        &mut screen.lock().unwrap(),
        &dir,
        agent,
        Provider::ClaudeCode,
    );
    worker
        .notify_dialogs(vec![(agent, Some(LATENCY_EPISODE.into()))])
        .unwrap();
    worker
        .store
        .fail_once_at(crate::messaging::storage::state_store::SaveStage::TempSync);
    assert!(worker
        .notify_dialogs(vec![(agent, Some(LATENCY_EPISODE.into()))])
        .is_err());
    assert_eq!(screen.lock().unwrap().inputs, 0);
    assert!(sent_by(&worker, room, agent).is_empty());
    assert!(worker
        .state
        .agent(agent)
        .unwrap()
        .automatic_dialog_answer
        .is_none());
    assert!(worker
        .store
        .load()
        .unwrap()
        .unwrap()
        .agent(agent)
        .unwrap()
        .automatic_dialog_answer
        .is_none());
}

#[test]
fn a_different_prompt_after_latency_failure_still_gets_ordinary_help() {
    let (mut worker, agent, room, _, screen, dir) = worker(true);
    let _scratch = AutomaticScratch(dir.clone());
    {
        let mut native = screen.lock().unwrap();
        configure_latency(&mut native, &dir, agent, Provider::ClaudeCode);
        native.press_uncertain = true;
    }
    notify_steady(&mut worker, agent, Some(LATENCY_EPISODE));
    assert_eq!(screen.lock().unwrap().inputs, 1);
    {
        let mut native = screen.lock().unwrap();
        native.episode_override = Some("new-command-approval".into());
        native.raw_screen = Some("Run this command?\n❯ Approve once\n  Deny".into());
        native.allow_driver_input = false;
    }
    notify_steady(&mut worker, agent, Some("new-command-approval"));
    assert_eq!(screen.lock().unwrap().inputs, 1);
    assert_eq!(sent_by(&worker, room, agent).len(), 2);
}

#[test]
fn latency_menu_after_a_distinct_prompt_starts_a_new_attempt_without_an_idle_snapshot() {
    for stalled in [false, true] {
        let (mut worker, agent, room, _, screen, dir) = worker(true);
        let _scratch = AutomaticScratch(dir.clone());
        {
            let mut native = screen.lock().unwrap();
            configure_latency(&mut native, &dir, agent, Provider::ClaudeCode);
            native.press_uncertain = !stalled;
            native.press_stalled = stalled;
        }
        notify_steady(&mut worker, agent, Some(LATENCY_EPISODE));
        assert_eq!(screen.lock().unwrap().inputs, 1);
        {
            let mut native = screen.lock().unwrap();
            native.episode_override = Some("intervening-command-approval".into());
            native.raw_screen = Some("Run this command?\n❯ Approve once\n  Deny".into());
            native.allow_driver_input = false;
        }
        notify_steady(&mut worker, agent, Some("intervening-command-approval"));
        assert_eq!(screen.lock().unwrap().inputs, 1);
        assert_eq!(sent_by(&worker, room, agent).len(), 2);

        // The native screen changed directly to another prompt and back. There
        // was no idle/clear observation and the runtime identity is unchanged.
        configure_latency(
            &mut screen.lock().unwrap(),
            &dir,
            agent,
            Provider::ClaudeCode,
        );
        notify_steady(&mut worker, agent, Some(LATENCY_EPISODE));
        assert_eq!(screen.lock().unwrap().inputs, 2, "stalled={stalled}");
        notify_steady(&mut worker, agent, Some(LATENCY_EPISODE));
        notify_steady(&mut worker, agent, Some(LATENCY_EPISODE));
        assert_eq!(
            screen.lock().unwrap().inputs,
            2,
            "one attempt in the new episode"
        );
        assert_eq!(sent_by(&worker, room, agent).len(), 3);
    }
}

#[test]
fn unidentified_blocked_fallback_does_not_renew_a_latency_attempt() {
    let (mut worker, agent, room, _, screen, dir) = worker(true);
    let _scratch = AutomaticScratch(dir.clone());
    {
        let mut native = screen.lock().unwrap();
        configure_latency(&mut native, &dir, agent, Provider::ClaudeCode);
        native.press_uncertain = true;
    }
    notify_steady(&mut worker, agent, Some(LATENCY_EPISODE));
    assert_eq!(screen.lock().unwrap().inputs, 1);
    let reserved = worker
        .state
        .agent(agent)
        .unwrap()
        .automatic_dialog_answer
        .clone();
    {
        let mut native = screen.lock().unwrap();
        native.dialog = false;
        native.blocked = true;
    }
    notify_steady(&mut worker, agent, Some(BLOCKED));
    assert_eq!(
        worker.state.agent(agent).unwrap().automatic_dialog_answer,
        reserved
    );
    configure_latency(
        &mut screen.lock().unwrap(),
        &dir,
        agent,
        Provider::ClaudeCode,
    );
    notify_steady(&mut worker, agent, Some(LATENCY_EPISODE));
    assert_eq!(screen.lock().unwrap().inputs, 1);
    assert_eq!(sent_by(&worker, room, agent).len(), 1);
}
