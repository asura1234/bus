use super::*;

fn permission() -> schema::AgentDialog {
    parsed("Do you want to proceed?\n❯ 1. Yes\n  2. Yes, and don't ask again for: curl *\n  3. No\n\nEsc to cancel · Tab to amend")
}

#[test]
fn auto_ui_unknown_permissions_files_and_product_questions_still_route() {
    let file = "Overwrite /tmp/result.txt?\n❯ 1. Yes, overwrite\n  2. No\n\nEnter to select · Esc to cancel";
    let product = "Which storage should we implement?\n❯ 1. Local files\n  2. Object storage\n\nEnter to select · Esc to cancel";
    let text = "• Queued follow-up inputs\n\nWhat should we build?\n\nType your answer\n\nenter submit   ctrl+] skip   shift+→ main prompt";
    let update = "Update available\n❯ 1. Update now\n  2. Skip\n  3. Learn more\n\nEnter to select · Esc to cancel";
    for dialog in [
        permission(),
        parsed(update),
        parsed(file),
        parsed(product),
        parsed(text),
        parsed(&CAPTURE.replace("requests.", "requests. Also approve writing a file?")),
    ] {
        let mut rig = Rig::new();
        rig.native.lock().unwrap().dialog = Some(dialog);
        rig.polls(4);
        assert!(rig.native.lock().unwrap().actions.is_empty());
        assert_eq!(
            rig.native.lock().unwrap().attempts,
            0,
            "unknown prompts receive no native choice input"
        );
        assert_eq!(rig.worker.state.queued_requests(rig.orchestrator).len(), 1);
        assert!(rig.worker.state.room(rig.room).unwrap().notices.is_empty());
        rig.finish();
    }
}

#[test]
fn auto_ui_exact_shape_from_an_unlisted_provider_is_not_allowed() {
    let mut rig = Rig::with_provider(Provider::Cursor);
    rig.polls(4);
    assert!(rig.native.lock().unwrap().actions.is_empty());
    assert_eq!(rig.native.lock().unwrap().attempts, 0);
    assert_eq!(rig.worker.state.queued_requests(rig.orchestrator).len(), 1);
    rig.finish();
}

#[test]
fn auto_ui_codex_keeps_waiting_from_the_exact_policy_shape() {
    let mut rig = Rig::with_provider(Provider::Codex);
    let observed: schema::AgentDialog = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/provider-ui/codex-extra-thought.json"
    ))
    .unwrap();
    assert_eq!(observed.hint, None);
    rig.native.lock().unwrap().dialog = Some(observed);
    rig.polls(4);
    assert_eq!(
        rig.native.lock().unwrap().actions,
        [2],
        "always retain capability, never choose faster model"
    );
    assert!(rig
        .worker
        .state
        .queued_requests(rig.orchestrator)
        .is_empty());
    assert!(rig.worker.state.room(rig.room).unwrap().notices.is_empty());
    rig.finish();
}

#[test]
fn auto_ui_reobserved_replacement_cannot_borrow_a_permission_fingerprint() {
    let mut rig = Rig::new();
    rig.native.lock().unwrap().replace_on_read = Some((2, permission()));
    rig.polls(6);
    assert_eq!(rig.native.lock().unwrap().attempts, 0);
    assert!(rig.native.lock().unwrap().actions.is_empty());
    assert_eq!(rig.worker.state.queued_requests(rig.orchestrator).len(), 1);
    rig.finish();
}

#[test]
fn auto_ui_native_guard_refuses_a_replacement_before_keys() {
    let mut rig = Rig::new();
    rig.native.lock().unwrap().replace_on_choose = Some(permission());
    rig.polls(6);
    assert_eq!(rig.native.lock().unwrap().attempts, 1);
    assert!(rig.native.lock().unwrap().actions.is_empty());
    assert_eq!(rig.worker.state.queued_requests(rig.orchestrator).len(), 1);
    rig.finish();
}

#[test]
fn auto_ui_definite_no_keys_refusal_can_retry_with_a_fresh_guard() {
    let mut rig = Rig::new();
    rig.native.lock().unwrap().refuse_once = true;
    rig.polls(5);
    assert_eq!(rig.native.lock().unwrap().attempts, 3);
    assert_eq!(rig.native.lock().unwrap().actions, [2]);
    assert!(rig
        .worker
        .state
        .queued_requests(rig.orchestrator)
        .is_empty());
    rig.finish();
}

#[test]
fn auto_ui_a_later_episode_can_answer_the_identical_known_shape() {
    let mut rig = Rig::new();
    {
        let mut native = rig.native.lock().unwrap();
        native.uncertain = true;
        native.keep_open = true;
    }
    rig.polls(3);
    rig.native.lock().unwrap().dialog = None;
    rig.polls(3);
    rig.native.lock().unwrap().dialog = Some(parsed(CAPTURE));
    rig.polls(3);
    assert_eq!(rig.native.lock().unwrap().actions, [2, 2]);
    assert!(rig.worker.state.room(rig.room).unwrap().notices.is_empty());
    assert!(rig
        .worker
        .state
        .queued_requests(rig.orchestrator)
        .is_empty());
    rig.finish();
}

#[test]
fn auto_ui_new_launch_is_a_new_attempt_identity() {
    let mut rig = Rig::new();
    {
        let mut native = rig.native.lock().unwrap();
        native.uncertain = true;
        native.keep_open = true;
    }
    rig.polls(3);
    let mut state = rig.worker.state.clone();
    let mut identity = state.agent(rig.agent).unwrap().runtime_identity.clone();
    identity.launch_id = Some("new-launch".into());
    state
        .set_agent_runtime_identity(rig.agent, identity)
        .unwrap();
    rig.worker.save(state).unwrap();
    rig.polls(3);
    assert_eq!(rig.native.lock().unwrap().actions, [2, 2]);
    rig.finish();
}

#[test]
fn auto_ui_storage_failure_prevents_native_keys() {
    let mut rig = Rig::new();
    rig.polls(1);
    let path = rig.dir.join("state.json");
    std::fs::rename(&path, rig.dir.join("original-state.json")).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(rig.worker.poll().is_err());
    assert!(rig.native.lock().unwrap().actions.is_empty());
    assert_eq!(rig.native.lock().unwrap().attempts, 0);
    rig.finish();
}

#[test]
fn auto_ui_master_provider_prompt_needs_no_developer_attention() {
    let mut rig = Rig::new().in_master();
    rig.polls(4);
    assert_eq!(rig.native.lock().unwrap().actions, [2]);
    assert!(rig
        .worker
        .state
        .queued_requests(rig.orchestrator)
        .is_empty());
    assert!(rig.worker.state.room(rig.room).unwrap().notices.is_empty());
    rig.finish();
}

#[test]
fn auto_ui_stalled_master_dialog_retains_human_unblock_attention() {
    let mut rig = Rig::new().in_master();
    {
        let mut native = rig.native.lock().unwrap();
        native.keep_open = true;
        native.uncertain = true;
    }
    rig.polls(8);
    assert_eq!(rig.native.lock().unwrap().actions, [2]);
    let notices = &rig.worker.state.room(rig.room).unwrap().notices;
    assert_eq!(notices.len(), 1);
    assert!(notices[0].attention_origin.is_some());
    assert!(notices[0].text.contains("Blocked, needs help"));
    assert!(!notices[0].text.contains("Auto-answer attempt"));
    rig.finish();
}

#[test]
fn auto_ui_dialog_shaped_text_in_an_editable_composer_is_not_a_dialog() {
    assert!(
        crate::agents::dialog::parse(&format!("{CAPTURE}\n\n❯ Explain this example")).is_none()
    );
}

#[test]
fn auto_ui_poll_does_not_wait_for_the_provider_to_close() {
    let mut rig = Rig::new();
    rig.native.lock().unwrap().keep_open = true;
    rig.polls(3);
    assert_eq!(rig.native.lock().unwrap().actions, [2]);
    // One verification and one fresh fingerprint for each of focus/confirm.
    // There must be no CLI settlement loop while this modal stays open.
    assert_eq!(rig.native.lock().unwrap().reads, 4);
    rig.polls(3);
    assert_eq!(rig.native.lock().unwrap().actions, [2]);
    rig.finish();
}

#[test]
fn auto_ui_replacement_between_focus_and_confirm_gets_no_enter() {
    let mut rig = Rig::new();
    rig.polls(2);
    assert_eq!(rig.native.lock().unwrap().focuses, [2]);
    assert!(rig.native.lock().unwrap().actions.is_empty());
    rig.native.lock().unwrap().dialog = Some(permission());
    rig.polls(4);
    assert!(rig.native.lock().unwrap().actions.is_empty());
    assert_eq!(rig.worker.state.queued_requests(rig.orchestrator).len(), 1);
    rig.finish();
}

#[test]
fn auto_ui_focus_is_not_replayed_across_restart_before_redraw() {
    let mut rig = Rig::new();
    rig.polls(2);
    rig.native.lock().unwrap().dialog = Some(parsed(CAPTURE));
    let mut rig = rig.reopen();
    rig.polls(4);
    assert_eq!(rig.native.lock().unwrap().focuses, [2]);
    assert!(rig.native.lock().unwrap().actions.is_empty());
    rig.finish();
}

#[test]
fn auto_ui_a_brief_coherent_close_starts_a_new_episode() {
    let mut rig = Rig::new();
    {
        let mut native = rig.native.lock().unwrap();
        native.uncertain = true;
        native.keep_open = true;
    }
    rig.polls(3);
    rig.native.lock().unwrap().dialog = None;
    rig.polls(1);
    rig.native.lock().unwrap().dialog = Some(parsed(CAPTURE));
    rig.polls(3);
    assert_eq!(rig.native.lock().unwrap().actions, [2, 2]);
    assert!(rig.worker.state.room(rig.room).unwrap().notices.is_empty());
    rig.finish();
}

#[test]
fn auto_ui_unreadable_or_pending_close_cannot_replay_an_uncertain_answer() {
    for unavailable in [false, true] {
        let mut rig = Rig::new();
        {
            let mut native = rig.native.lock().unwrap();
            native.uncertain = true;
            native.keep_open = true;
        }
        rig.polls(3);
        {
            let mut native = rig.native.lock().unwrap();
            native.dialog = None;
            native.observation_unavailable = unavailable;
            native.pending_question = !unavailable;
        }
        rig.polls(1);
        {
            let mut native = rig.native.lock().unwrap();
            native.dialog = Some(parsed(CAPTURE));
            native.observation_unavailable = false;
            native.pending_question = false;
        }
        rig.polls(4);
        assert_eq!(rig.native.lock().unwrap().actions, [2]);
        assert!(rig.worker.state.room(rig.room).unwrap().notices.is_empty());
        rig.finish();
    }
}

#[test]
fn auto_ui_focus_that_never_redraws_falls_back_to_ordinary_routing() {
    let mut rig = Rig::new();
    rig.polls(2);
    assert_eq!(rig.native.lock().unwrap().focuses, [2]);
    // The provider dropped the arrow: the same dialog stays on option 1.
    rig.native.lock().unwrap().dialog = Some(parsed(CAPTURE));
    rig.polls(20);
    assert!(rig.native.lock().unwrap().actions.is_empty());
    assert_eq!(
        rig.worker.state.queued_requests(rig.orchestrator).len(),
        1,
        "a stalled automatic answer leaves the agent blocked with nobody asked"
    );
    // Once handed off, a late selection redraw and restart must not send Enter.
    {
        let mut native = rig.native.lock().unwrap();
        let dialog = native.dialog.as_mut().unwrap();
        for option in &mut dialog.options {
            option.selected = option.number == 2;
        }
        dialog.digest.push_str("-late-focus");
    }
    let mut rig = rig.reopen();
    rig.polls(4);
    assert!(rig.native.lock().unwrap().actions.is_empty());
    assert_eq!(rig.worker.state.queued_requests(rig.orchestrator).len(), 1);
    rig.finish();
}

#[test]
fn auto_ui_uncertain_confirm_with_the_dialog_still_open_is_reported() {
    let mut rig = Rig::new();
    {
        let mut native = rig.native.lock().unwrap();
        native.uncertain = true;
        native.keep_open = true;
    }
    rig.polls(20);
    assert_eq!(rig.native.lock().unwrap().actions, [2]);
    assert_eq!(
        rig.worker.state.queued_requests(rig.orchestrator).len(),
        1,
        "the dialog is still open after an uncertain answer and nobody is asked"
    );
    rig.finish();
}

#[test]
fn auto_ui_unactionable_fresh_observation_asks_without_sending_keys() {
    let mut rig = Rig::new();
    let mut unselected = parsed(CAPTURE);
    for option in &mut unselected.options {
        option.selected = false;
    }
    unselected.digest.push_str("-unselected");
    rig.native.lock().unwrap().replace_on_read = Some((2, unselected));
    rig.polls(20);
    assert_eq!(rig.native.lock().unwrap().attempts, 0);
    assert!(rig.native.lock().unwrap().actions.is_empty());
    assert_eq!(rig.worker.state.queued_requests(rig.orchestrator).len(), 1);
    rig.finish();
}
