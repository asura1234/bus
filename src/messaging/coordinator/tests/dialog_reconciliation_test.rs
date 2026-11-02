use super::*;
use crate::messaging::model::DialogNoticeTo;

fn unconfirmed_worker_notice() -> (Fixture, AgentId, RequestId, RequestId) {
    let (mut f, worker, lead, notice) = queued_worker_notice();
    let mut state = f.worker.state.clone();
    state.begin_steering(notice, lead).unwrap();
    state.begin_dialog_notice_write(notice, f.request(notice).prompt.submitted_at_ms);
    state
        .record_steering(
            notice,
            SubmissionOutcome::Uncertain {
                message: "native response timed out after the write".into(),
            },
        )
        .unwrap();
    f.worker.save(state).unwrap();
    (f, worker, lead, notice)
}

fn queued_worker_notice() -> (Fixture, AgentId, RequestId, RequestId) {
    let mut f = Fixture::in_room(Provider::ClaudeCode, true);
    let mut state = f.worker.state.clone();
    let work = state.create_room("blocked-work").unwrap();
    state.bind_orchestrator(f.agent, work).unwrap();
    let worker = state
        .create_agent(work, "blocked-worker", Provider::Codex, f.dir.clone(), None)
        .unwrap();
    f.worker.save(state).unwrap();
    f.bind_dialog_pane(worker);
    f.record(json!({"hook_event_name":"SessionStart","session_id":"session","source":"startup"}));
    f.consume();
    let lead = running(&mut f, "original task", "original-turn");
    for _ in 0..2 {
        f.worker
            .notify_dialogs(vec![(
                worker,
                Some(crate::messaging::coordinator::dialogs::BLOCKED.into()),
            )])
            .unwrap();
    }
    let Some(DialogNoticeTo::Orchestrator(notice)) =
        f.worker.state.agent(worker).unwrap().dialog_notice_to
    else {
        panic!("the worker must have a routed dialog notice");
    };
    (f, worker, lead, notice)
}

#[test]
fn a_durable_start_hook_prevents_an_unconfirmed_notice_fallback_on_the_same_tick() {
    let (mut f, worker, _, notice) = unconfirmed_worker_notice();
    let payload = f.worker.state.render_request_payload(notice).unwrap();
    // The provider accepted the notice and its matching start is already on
    // disk. Reproduce tick's expiry-before-callback-reconciliation ordering.
    f.started("original-turn", &payload);
    let deadline =
        f.request(notice).prompt.submitted_at_ms + crate::messaging::model::QUEUED_STALL_MS;
    f.worker.expire_queued_requests(deadline).unwrap();
    f.consume();
    f.worker.expire_unconfirmed_submissions(deadline).unwrap();
    assert!(f.request(notice).trusted_start_bound);
    assert!(!f.request(notice).uncertain_outcome);
    assert_eq!(
        f.worker.state.agent(worker).unwrap().dialog_notice_to,
        Some(DialogNoticeTo::Orchestrator(notice)),
        "a matching durable start must be reconciled before escalating an unknown write"
    );
}

#[test]
fn an_unconfirmed_notice_still_falls_back_after_its_lead_finishes_without_binding_it() {
    let (mut f, worker, lead, notice) = unconfirmed_worker_notice();
    f.stopped("original-turn", "original task completed");
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.now = crate::messaging::storage::io::now_ms() + 5_000 + 1;
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_eq!(f.request(lead).phase, RequestPhase::Completed);
    assert_eq!(f.request(notice).phase, RequestPhase::Abandoned);
    assert!(f.request(notice).uncertain_outcome);
    assert!(!f.request(notice).trusted_start_bound);
    let deadline =
        f.request(notice).prompt.submitted_at_ms + crate::messaging::model::QUEUED_STALL_MS;
    f.expire_after_reconciliation(deadline);
    assert_eq!(
        f.worker.state.agent(worker).unwrap().dialog_notice_to,
        Some(DialogNoticeTo::Developer),
        "an unrelated lead answer cannot confirm delivery of the blocked worker's notice"
    );
}

#[test]
fn a_reconciled_notice_never_falls_back_after_its_original_deadline() {
    let (mut f, worker, _, notice) = unconfirmed_worker_notice();
    let payload = f.worker.state.render_request_payload(notice).unwrap();
    f.started("original-turn", &payload);
    f.consume();
    assert!(f.request(notice).trusted_start_bound);
    let deadline =
        f.request(notice).prompt.submitted_at_ms + crate::messaging::model::QUEUED_STALL_MS;
    f.expire_after_reconciliation(deadline);
    f.expire_after_reconciliation(deadline + 60_000);
    assert_eq!(
        f.worker.state.agent(worker).unwrap().dialog_notice_to,
        Some(DialogNoticeTo::Orchestrator(notice))
    );
    assert!(f
        .worker
        .state
        .room(f.request(notice).room_id)
        .unwrap()
        .notices
        .is_empty());
}

#[test]
fn an_escalated_uncertain_notice_is_not_reposted_after_state_reload() {
    let (mut f, worker, _, notice) = unconfirmed_worker_notice();
    let deadline =
        f.request(notice).prompt.submitted_at_ms + crate::messaging::model::QUEUED_STALL_MS;
    f.expire_after_reconciliation(deadline);
    let mut reloaded = f.worker.store.load().unwrap().unwrap();
    reloaded
        .expire_stalled_queued_requests(deadline + 60_000)
        .unwrap();
    f.worker.save(reloaded).unwrap();
    f.expire_after_reconciliation(deadline + 60_000);
    let reloaded = f.worker.store.load().unwrap().unwrap();
    assert_eq!(
        reloaded.agent(worker).unwrap().dialog_notice_to,
        Some(DialogNoticeTo::Developer)
    );
    assert_eq!(
        reloaded
            .room(f.request(notice).room_id)
            .unwrap()
            .notices
            .len(),
        1
    );
}

#[test]
fn a_late_write_gets_its_full_bound_even_after_restart_without_an_outcome() {
    let (mut f, worker, lead, notice) = queued_worker_notice();
    let created = f.request(notice).prompt.submitted_at_ms;
    let write_at = created + crate::messaging::model::QUEUED_STALL_MS - 1;
    let mut state = f.worker.state.clone();
    state.begin_steering(notice, lead).unwrap();
    state.begin_dialog_notice_write(notice, write_at);
    f.worker.save(state).unwrap();
    let reloaded = f.worker.store.load().unwrap().unwrap();
    f.worker.save(reloaded).unwrap();
    assert!(!f.request(notice).uncertain_outcome);
    let deadline = write_at + crate::messaging::model::QUEUED_STALL_MS;
    for at in [
        created + crate::messaging::model::QUEUED_STALL_MS,
        deadline - 1,
    ] {
        f.expire_after_reconciliation(at);
        assert_eq!(
            f.worker.state.agent(worker).unwrap().dialog_notice_to,
            Some(DialogNoticeTo::Orchestrator(notice))
        );
    }
    f.expire_after_reconciliation(deadline);
    assert_eq!(
        f.worker.state.agent(worker).unwrap().dialog_notice_to,
        Some(DialogNoticeTo::Developer)
    );
    let room = f.worker.state.room(f.request(notice).room_id).unwrap();
    assert_eq!(room.notices.len(), 1);
    assert!(room.notices[0]
        .text
        .contains("Delivery to the orchestrator is unconfirmed"));
    assert_eq!(f.request(notice).phase, RequestPhase::Submitting);
    assert_eq!(f.request(notice).group, Some(lead));
}

#[test]
fn native_confirmation_survives_group_completion_and_reload_without_a_start_hook() {
    let (mut f, worker, _, notice) = queued_worker_notice();
    f.deliver();
    assert_eq!(
        f.request(notice).dialog_delivery,
        Some(crate::messaging::model::DialogNoticeDelivery::Confirmed)
    );
    assert!(!f.request(notice).trusted_start_bound);
    f.stopped("original-turn", "original task completed");
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.now = crate::messaging::storage::io::now_ms() + 5_000 + 1;
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_eq!(f.request(notice).phase, RequestPhase::Abandoned);
    let state = f.worker.store.load().unwrap().unwrap();
    f.worker.save(state).unwrap();
    f.expire_after_reconciliation(
        f.request(notice).prompt.submitted_at_ms + crate::messaging::model::QUEUED_STALL_MS,
    );
    assert_eq!(
        f.worker.state.agent(worker).unwrap().dialog_notice_to,
        Some(DialogNoticeTo::Orchestrator(notice))
    );
    assert!(f
        .worker
        .state
        .room(f.request(notice).room_id)
        .unwrap()
        .notices
        .is_empty());
}

#[test]
fn a_closed_dialog_cancels_an_unconfirmed_notice_fallback() {
    let (mut f, worker, _, notice) = unconfirmed_worker_notice();
    f.worker.finish_closed_dialog(worker).unwrap();
    f.expire_after_reconciliation(
        f.request(notice).prompt.submitted_at_ms + crate::messaging::model::QUEUED_STALL_MS,
    );
    assert_eq!(f.worker.state.agent(worker).unwrap().dialog_notice_to, None);
    assert!(f
        .worker
        .state
        .room(f.request(notice).room_id)
        .unwrap()
        .notices
        .is_empty());
}

#[test]
fn an_unconfirmed_new_turn_write_uses_the_same_episode_fallback() {
    let (mut f, worker, lead, notice) = queued_worker_notice();
    f.stopped("original-turn", "done");
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_eq!(f.request(lead).phase, RequestPhase::Completed);
    let at = f.request(notice).prompt.submitted_at_ms + 30_000;
    let mut state = f.worker.state.clone();
    state.begin_submission(notice, "launch", 0).unwrap();
    state.begin_dialog_notice_write(notice, at);
    f.worker.save(state).unwrap();
    let state = f.worker.store.load().unwrap().unwrap();
    f.worker.save(state).unwrap();
    f.expire_after_reconciliation(at + crate::messaging::model::QUEUED_STALL_MS);
    assert_eq!(
        f.worker.state.agent(worker).unwrap().dialog_notice_to,
        Some(DialogNoticeTo::Developer)
    );
    assert_eq!(f.request(notice).phase, RequestPhase::Submitting);
}

#[test]
fn a_settled_notice_remains_eligible_after_archival_and_reload() {
    let (mut f, worker, _, notice) = unconfirmed_worker_notice();
    f.stopped("original-turn", "done");
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.now = crate::messaging::storage::io::now_ms() + 5_000 + 1;
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_eq!(f.request(notice).phase, RequestPhase::Abandoned);
    let now = f.now + crate::messaging::model::ARCHIVE_AFTER_MS + 1;
    let mut state = f.worker.state.clone();
    state.archive(now);
    f.worker.save(state).unwrap();
    let state = f.worker.store.load().unwrap().unwrap();
    f.worker.save(state).unwrap();
    assert_eq!(f.request(notice).phase, RequestPhase::Abandoned);
    f.expire_after_reconciliation(now);
    assert_eq!(
        f.worker.state.agent(worker).unwrap().dialog_notice_to,
        Some(DialogNoticeTo::Developer)
    );
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
fn losing_the_orchestrator_binding_does_not_cancel_an_open_episode_fallback() {
    let (mut f, worker, _, notice) = unconfirmed_worker_notice();
    let mut state = f.worker.state.clone();
    state.prepare_delete_agent(f.agent).unwrap();
    f.worker.save(state).unwrap();
    f.expire_after_reconciliation(
        f.request(notice).prompt.submitted_at_ms + crate::messaging::model::QUEUED_STALL_MS,
    );
    assert_eq!(
        f.worker.state.agent(worker).unwrap().dialog_notice_to,
        Some(DialogNoticeTo::Developer)
    );
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
fn resuming_a_legacy_native_confirmation_preserves_its_receipt() {
    let (mut f, worker, _, notice) = queued_worker_notice();
    f.deliver();
    assert_eq!(f.request(notice).phase, RequestPhase::Active);
    assert!(!f.request(notice).uncertain_outcome);
    assert!(!f.request(notice).trusted_start_bound);
    let mut legacy = serde_json::to_value(&f.worker.state).unwrap();
    legacy["requests"][notice.0.to_string()]
        .as_object_mut()
        .unwrap()
        .remove("dialog_delivery");
    let legacy: crate::messaging::model::BusState = serde_json::from_value(legacy).unwrap();
    let resume = f.dir.join("legacy-resume");
    JsonStore::new(resume.join("state.json"))
        .save(&legacy)
        .unwrap();
    let mut resumed = Worker::open(resume, Box::new(Native(f.native.clone()))).unwrap();
    resumed
        .expire_unconfirmed_submissions(
            f.request(notice).prompt.submitted_at_ms + crate::messaging::model::QUEUED_STALL_MS,
        )
        .unwrap();
    assert_eq!(
        resumed.state.agent(worker).unwrap().dialog_notice_to,
        Some(DialogNoticeTo::Orchestrator(notice))
    );
    assert_eq!(
        resumed.state.request(notice).unwrap().dialog_delivery,
        Some(crate::messaging::model::DialogNoticeDelivery::Confirmed)
    );
}

#[test]
fn resuming_a_legacy_unknown_write_does_not_invent_a_confirmation() {
    for uncertain in [false, true] {
        let (f, worker, lead, notice) = queued_worker_notice();
        let mut state = f.worker.state.clone();
        state.begin_steering(notice, lead).unwrap();
        if uncertain {
            state
                .record_steering(
                    notice,
                    SubmissionOutcome::Uncertain {
                        message: "lost native outcome".into(),
                    },
                )
                .unwrap();
        }
        assert_eq!(state.request(notice).unwrap().dialog_delivery, None);
        let resume = f.dir.join("legacy-unknown-resume");
        JsonStore::new(resume.join("state.json"))
            .save(&state)
            .unwrap();
        let mut resumed = Worker::open(resume, Box::new(Native(f.native.clone()))).unwrap();
        assert_eq!(resumed.state.request(notice).unwrap().dialog_delivery, None);
        resumed
            .expire_unconfirmed_submissions(
                f.request(notice).prompt.submitted_at_ms + crate::messaging::model::QUEUED_STALL_MS,
            )
            .unwrap();
        assert_eq!(
            resumed.state.agent(worker).unwrap().dialog_notice_to,
            Some(DialogNoticeTo::Developer)
        );
    }
}

#[test]
fn an_unreadable_uncertain_notice_uses_its_write_clock_and_preserves_delivery_uncertainty() {
    struct Unreadable;
    impl Transport for Unreadable {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            assert!(
                matches!(method, Method::AgentGet(_)),
                "must not replay a native write"
            );
            Err(TransportError {
                code: Some("terminal_observation_unavailable".into()),
                message: "Agent screen remains unreadable".into(),
                definitely_rejected: true,
            })
        }
    }
    let (mut f, worker, lead, notice) = queued_worker_notice();
    let created = f.request(notice).prompt.submitted_at_ms;
    let write_at = created + crate::messaging::model::QUEUED_STALL_MS - 1;
    let mut state = f.worker.state.clone();
    state.begin_steering(notice, lead).unwrap();
    state.begin_dialog_notice_write(notice, write_at);
    state
        .record_steering(
            notice,
            SubmissionOutcome::Uncertain {
                message: "native response timed out after the write".into(),
            },
        )
        .unwrap();
    f.worker.save(state).unwrap();
    f.worker.transport = Box::new(Unreadable);
    let grace = crate::messaging::coordinator::expiry::DIALOG_OBSERVATION_GRACE_MS;
    let bound = write_at + crate::messaging::model::QUEUED_STALL_MS + grace;
    for at in [
        created + crate::messaging::model::QUEUED_STALL_MS + grace,
        bound - 1,
    ] {
        let reloaded = f.worker.store.load().unwrap().unwrap();
        f.worker.save(reloaded).unwrap();
        f.expire_after_reconciliation(at);
        assert_eq!(
            f.worker.state.agent(worker).unwrap().dialog_notice_to,
            Some(DialogNoticeTo::Orchestrator(notice))
        );
        assert!(f
            .worker
            .state
            .room(f.request(notice).room_id)
            .unwrap()
            .notices
            .is_empty());
    }
    f.expire_after_reconciliation(bound);
    f.expire_after_reconciliation(bound + 86_400_000);
    assert_eq!(
        f.worker.state.agent(worker).unwrap().dialog_notice_to,
        Some(DialogNoticeTo::Developer)
    );
    assert_eq!(f.request(notice).phase, RequestPhase::Active);
    assert_eq!(f.request(notice).group, Some(lead));
    let room = f.worker.state.room(f.request(notice).room_id).unwrap();
    assert_eq!(room.notices.len(), 1);
    let text = &room.notices[0].text;
    assert!(text.contains(
        "Delivery to the orchestrator is unconfirmed; it may already have received this"
    ));
    assert!(text.contains("The dialog state could not be read; the agent may still be blocked."));
}
