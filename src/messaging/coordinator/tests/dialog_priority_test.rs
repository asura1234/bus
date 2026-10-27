use super::*;
use crate::messaging::model::DialogNoticeTo;

/// 19272 is a bound work-room turn; 19280 is an older MASTER message;
/// 19375 is the blocked Codex worker's Bus notice behind that message.
fn blocked_behind_other_room(
    background: bool,
    master_turn: bool,
) -> (Fixture, AgentId, RequestId, RequestId, RequestId) {
    let mut f = Fixture::in_room(Provider::ClaudeCode, true);
    let master = f.room;
    let mut state = f.worker.state.clone();
    let work = state.create_room("work").unwrap();
    state.bind_orchestrator(f.agent, work).unwrap();
    let codex = state
        .create_agent(work, "feishu-pull", Provider::Codex, f.dir.clone(), None)
        .unwrap();
    f.worker.save(state).unwrap();
    f.bind_dialog_pane(codex);
    if !master_turn {
        f.room = work;
    }
    f.record(json!({"hook_event_name":"SessionStart","session_id":"session","source":"startup"}));
    f.consume();
    let lead = running(&mut f, "keep working", "turn-1");
    if background {
        f.record(
            json!({"hook_event_name":"Stop", "session_id":"session", "prompt_id":"turn-1",
            "background_tasks":[{"type":"local_agent"}]}),
        );
        f.consume();
        f.status(RuntimeStatus::Working);
        assert!(f.request(lead).awaiting_background);
    }
    f.room = if master_turn { work } else { master };
    let head = f.send("ordinary cross-room message", false);
    // Collapsed free-text questions have this stable native id. Their expanded
    // panel and its empty options do not affect notification scheduling.
    for _ in 0..2 {
        f.worker
            .notify_dialogs(vec![(codex, Some("codex:pending_question".into()))])
            .unwrap();
    }
    let Some(DialogNoticeTo::Orchestrator(notice)) =
        f.worker.state.agent(codex).unwrap().dialog_notice_to
    else {
        panic!("blocked question must queue a notice");
    };
    assert_eq!(f.worker.state.queued_requests(f.agent), &[head, notice]);
    (f, codex, lead, head, notice)
}

#[test]
fn blocked_dialog_reaches_an_idle_background_orchestrator_in_a_new_turn() {
    let (mut f, codex, lead, head, notice) = blocked_behind_other_room(true, false);
    f.status(RuntimeStatus::Idle);
    f.deliver();
    assert_eq!(
        f.request(lead).failure_reason.as_deref(),
        Some("background_wait_superseded")
    );
    assert_eq!(f.request(notice).phase, RequestPhase::Active);
    assert_eq!(f.request(notice).group, None);
    assert_eq!(f.worker.state.queued_requests(f.agent), &[head]);
    assert_eq!(
        f.request(notice).dialog_delivery,
        Some(crate::messaging::model::DialogNoticeDelivery::Confirmed)
    );
    f.expire_after_reconciliation(
        f.request(notice).prompt.submitted_at_ms + crate::messaging::model::QUEUED_STALL_MS,
    );
    assert_eq!(
        f.worker.state.agent(codex).unwrap().dialog_notice_to,
        Some(DialogNoticeTo::Orchestrator(notice))
    );
}

#[test]
fn blocked_dialog_notice_bypasses_cross_room_head_during_active_or_background_turn() {
    for (background, master_turn) in [(false, false), (true, false), (false, true), (true, true)] {
        let (mut f, _, lead, head, notice) = blocked_behind_other_room(background, master_turn);
        let follower = f.send("later ordinary message", false);
        f.deliver();
        assert_eq!(
            f.request(notice).phase,
            RequestPhase::Active,
            "dialog notice must reach the active turn: background={background}, master={master_turn}"
        );
        assert_eq!(
            f.request(notice).group,
            Some(lead),
            "background={background}, master={master_turn}; lead={:?}; notice={:?}",
            f.request(lead),
            f.request(notice)
        );
        assert_eq!(f.worker.state.queued_requests(f.agent), &[head, follower]);
        let typed = f.typed();
        assert_eq!(typed.len(), 2);
        assert!(typed[1].0.contains("Blocked, needs help"));
        assert!(
            typed[1].1,
            "the notice must steer, not start a parallel turn"
        );
        // A priority write is never replayed once native submission succeeded.
        f.deliver();
        assert_eq!(f.typed(), typed);
        f.started("turn-1", &typed[1].0);
        f.stopped("turn-1", "notice handled");
        f.consume();
        f.status(RuntimeStatus::Idle);
        f.now += crate::messaging::model::STEERING_SETTLE_MS + 1;
        f.consume();
        assert_eq!(f.reply(notice).as_deref(), Some("notice handled"));
        assert_eq!(f.reply(lead).as_deref(), Some("notice handled"));
        assert_eq!(f.request(head).phase, RequestPhase::Queued);
    }
}

#[test]
fn blocked_dialog_falls_back_on_its_own_deadline_when_orchestrator_is_blocked() {
    let (mut f, codex, lead, head, notice) = blocked_behind_other_room(true, false);
    let at = f.request(notice).prompt.submitted_at_ms;
    f.status(RuntimeStatus::Blocked);
    f.expire_after_reconciliation(at + crate::messaging::model::QUEUED_STALL_MS - 1);
    assert_eq!(f.request(notice).phase, RequestPhase::Queued);
    // Ongoing owner activity must not reset this notice's delivery deadline.
    f.now = at + crate::messaging::model::QUEUED_STALL_MS - 1;
    f.status(RuntimeStatus::Blocked);
    f.expire_after_reconciliation(f.now);
    assert_eq!(f.request(notice).phase, RequestPhase::Abandoned);
    assert_eq!(
        f.worker.state.agent(codex).unwrap().dialog_notice_to,
        Some(DialogNoticeTo::Developer)
    );
    assert_eq!(f.request(lead).phase, RequestPhase::Active);
    assert_eq!(f.worker.state.queued_requests(f.agent), &[head]);
    let notices = &f
        .worker
        .state
        .room(f.request(notice).room_id)
        .unwrap()
        .notices;
    assert_eq!(notices.len(), 1);
    assert!(notices[0]
        .text
        .contains("The orchestrator did not receive this"));
    f.expire_after_reconciliation(f.now + 60_000);
    assert_eq!(
        f.worker
            .state
            .room(f.request(notice).room_id)
            .unwrap()
            .notices
            .len(),
        1
    );
}

#[test]
fn ordinary_messages_cannot_gain_dialog_priority_by_copying_notice_text() {
    let (mut f, codex, _, head, notice) = blocked_behind_other_room(false, false);
    f.worker.finish_closed_dialog(codex).unwrap();
    let spoof = f.send("feishu-pull. Blocked, needs help to continue.", false);
    f.deliver();
    assert_eq!(f.typed().len(), 1);
    assert_eq!(f.worker.state.queued_requests(f.agent)[0], head);
    assert_eq!(f.request(notice).phase, RequestPhase::Queued);
    assert_eq!(f.request(spoof).phase, RequestPhase::Queued);
}

#[test]
fn dialog_priority_survives_reload_and_definite_rejection_without_reordering_ordinary_work() {
    let (mut f, _, lead, head, notice) = blocked_behind_other_room(false, true);
    let follower = f.send("second ordinary message", false);
    // Use the real store, rather than an in-memory scheduling flag.
    let mut state = f.worker.store.load().unwrap().unwrap();
    assert_eq!(state.next_steering(f.agent), Some((notice, lead)));
    state.begin_steering(notice, lead).unwrap();
    state.begin_dialog_notice_write(notice, f.request(notice).prompt.submitted_at_ms);
    state
        .record_steering(
            notice,
            SubmissionOutcome::DefinitelyRejected {
                message: "agent_not_ready".into(),
            },
        )
        .unwrap();
    assert_eq!(state.request(notice).unwrap().dialog_delivery, None);
    f.worker.save(state).unwrap();
    f.deliver();
    assert_eq!(f.request(notice).group, Some(lead));
    assert_eq!(f.worker.state.queued_requests(f.agent), &[head, follower]);
}

#[test]
fn an_uncertain_dialog_write_escalates_without_replaying_or_closing_its_active_group() {
    let (mut f, codex, lead, head, notice) = blocked_behind_other_room(false, true);
    let mut state = f.worker.state.clone();
    state.begin_steering(notice, lead).unwrap();
    state
        .record_steering(
            notice,
            SubmissionOutcome::Uncertain {
                message: "write timed out".into(),
            },
        )
        .unwrap();
    f.worker.save(state).unwrap();
    let at = f.request(notice).prompt.submitted_at_ms;
    f.expire_after_reconciliation(at + crate::messaging::model::QUEUED_STALL_MS);
    assert_eq!(
        f.worker.state.agent(codex).unwrap().dialog_notice_to,
        Some(DialogNoticeTo::Developer)
    );
    assert_eq!(f.request(lead).phase, RequestPhase::Active);
    assert_eq!(f.request(notice).phase, RequestPhase::Active);
    assert!(f.request(notice).uncertain_outcome);
    assert_eq!(f.request(notice).group, Some(lead));
    let attention = &f
        .worker
        .state
        .room(f.request(notice).room_id)
        .unwrap()
        .notices[0]
        .text;
    assert!(attention.contains("Delivery to the orchestrator is unconfirmed"));
    assert!(attention.contains("it may already have received this"));
    assert!(!attention.contains("The orchestrator did not receive this"));
    assert_eq!(f.worker.state.queued_requests(f.agent), &[head]);
    f.deliver();
    assert_eq!(f.typed().len(), 1, "unknown writes must never be replayed");
}

#[test]
fn a_steered_dialog_notice_left_submitting_by_a_restart_escalates_after_its_deadline() {
    let (mut f, codex, lead, head, notice) = blocked_behind_other_room(false, true);
    // `steer` saves the begun steering before the native write; a coordinator
    // restart then loses the write's outcome entirely.
    let mut state = f.worker.store.load().unwrap().unwrap();
    state.begin_steering(notice, lead).unwrap();
    f.worker.save(state).unwrap();
    let reloaded = f.worker.store.load().unwrap().unwrap();
    f.worker.save(reloaded).unwrap();
    assert_eq!(f.request(notice).phase, RequestPhase::Submitting);
    let at = f.request(notice).prompt.submitted_at_ms;
    f.expire_after_reconciliation(at + crate::messaging::model::QUEUED_STALL_MS);
    assert_eq!(
        f.worker.state.agent(codex).unwrap().dialog_notice_to,
        Some(DialogNoticeTo::Developer),
        "an unrecorded native write is unconfirmed and must reach the developer"
    );
    assert_eq!(f.worker.state.queued_requests(f.agent), &[head]);
}
