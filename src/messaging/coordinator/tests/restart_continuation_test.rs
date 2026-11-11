use super::*;

#[test]
fn restart_epoch_bound_codex_and_claude_resume_original_request_and_group() {
    for provider in [Provider::Codex, Provider::ClaudeCode] {
        let mut f = Fixture::new(provider);
        observe_epoch(&mut f, 1);
        let lead = running(&mut f, "ORIGINAL_TASK_DO_NOT_REPLAY", "turn-1");
        let member = f.send("ORIGINAL_STEERING_DO_NOT_REPLAY", false);
        f.deliver();
        f.started("turn-1", "ORIGINAL_STEERING_DO_NOT_REPLAY");
        f.consume();
        assert!(f.request(member).trusted_start_bound);
        let queued = f.send("independent task after restart", true);
        let message = f.request(lead).prompt.id;
        let member_message = f.request(member).prompt.id;
        let before = f.typed().len();

        epoch_reopen(&mut f, true, 2);
        f.worker.tick().unwrap();
        assert!(
            !f.request(lead).settled(),
            "{provider:?}: {:?}",
            f.request(lead)
        );
        assert!(
            !f.request(member).settled(),
            "{provider:?}: {:?}",
            f.request(member)
        );
        assert_eq!(f.request(member).group, Some(lead));
        assert_eq!(
            f.worker.state.agent(f.agent).unwrap().current_request,
            Some(lead)
        );
        assert_eq!(f.request(queued).phase, RequestPhase::Queued);
        let typed = f.typed();
        assert_eq!(
            typed.len(),
            before + 1,
            "one continuation, no queued-task paste"
        );
        let notice = typed.last().unwrap().0.clone();
        assert!(notice.contains("Bus restarted"), "{notice}");
        assert!(
            notice.contains(&format!("message {}", message.0)),
            "{notice}"
        );
        assert!(!notice.contains("ORIGINAL_TASK_DO_NOT_REPLAY"), "{notice}");
        assert!(
            !notice.contains("ORIGINAL_STEERING_DO_NOT_REPLAY"),
            "{notice}"
        );
        assert_eq!(f.request(lead).prompt.text, "ORIGINAL_TASK_DO_NOT_REPLAY");

        // A late old Stop must not complete the new execution owner.
        f.stopped("turn-1", "old interrupted final");
        f.consume();
        assert!(!f.request(lead).settled());
        f.started("turn-2", &notice);
        f.consume();
        assert_eq!(f.request(lead).provider_turn_id.as_deref(), Some("turn-2"));
        assert!(f.request(lead).trusted_start_bound);
        f.status(RuntimeStatus::Working);
        let reply = f.worker.control_response_with_events(
            &crate::messaging::control::Request {
                id: "restart-original-reply".into(),
                method: "message.send".into(),
                params: json!({"reply_to":message.0.to_string(), "as":f.agent.0.to_string(), "text":"original request answer"}),
            },
            None,
        );
        assert!(reply.ok, "{reply:?}");
        assert_eq!(reply.result["stage"], "replied");
        assert_eq!(f.reply(lead).as_deref(), Some("original request answer"));
        let reply = f.worker.control_response_with_events(
            &crate::messaging::control::Request {
                id: "restart-original-member-reply".into(),
                method: "message.send".into(),
                params: json!({"reply_to":member_message.0.to_string(), "as":f.agent.0.to_string(), "text":"original steering answer"}),
            },
            None,
        );
        assert!(reply.ok, "{reply:?}");
        f.deliver();
        assert_eq!(f.request(queued).phase, RequestPhase::Queued);
        f.stopped("turn-2", "native final is not a reply");
        f.consume();
        assert_eq!(f.request(lead).phase, RequestPhase::Completed);
        assert_eq!(f.request(member).phase, RequestPhase::Completed);
        assert_eq!(f.reply(member).as_deref(), Some("original steering answer"));
        f.status(RuntimeStatus::Idle);
        f.deliver();
        assert_eq!(f.request(queued).phase, RequestPhase::Active);
        assert_eq!(
            f.worker.state.requests().count(),
            3,
            "no substitute request IDs"
        );
    }
}

#[test]
fn restart_epoch_resumed_native_end_settles_original_unanswered_group() {
    for provider in [Provider::Codex, Provider::ClaudeCode] {
        let mut f = Fixture::new(provider);
        observe_epoch(&mut f, 1);
        let lead = running(&mut f, "unfinished original task", "turn-1");
        let member = f.send("unfinished joined input", false);
        f.deliver();
        f.started("turn-1", "unfinished joined input");
        f.consume();
        epoch_reopen(&mut f, true, 2);
        f.worker.tick().unwrap();
        assert!(
            !f.request(lead).settled(),
            "{provider:?}: {:?}",
            f.request(lead)
        );
        let notice = f.typed().last().unwrap().0.clone();
        f.started("turn-2", &notice);
        f.consume();
        f.stopped("turn-2", "completed resumed task");
        f.consume();
        assert_completed_without_reply(&f, lead);
        assert_completed_without_reply(&f, member);
        assert_eq!(f.worker.state.agent(f.agent).unwrap().current_request, None);
    }
}

#[test]
fn restart_epoch_unconfirmed_resume_notice_closes_original_group_at_normal_bound() {
    let mut f = Fixture::new(Provider::Codex);
    observe_epoch(&mut f, 1);
    let lead = running(&mut f, "interrupted task", "turn-1");
    let member = f.send("joined input", false);
    f.deliver();
    f.started("turn-1", "joined input");
    f.consume();
    epoch_reopen(&mut f, true, 2);
    f.worker.tick().unwrap();
    assert!(!f.request(lead).settled(), "{:?}", f.request(lead));
    f.expire_after_reconciliation(io::now_ms() + 10_000);
    for id in [lead, member] {
        assert_eq!(f.request(id).phase, RequestPhase::Abandoned);
        assert_eq!(
            f.request(id).failure_reason.as_deref(),
            Some("reply_not_captured")
        );
    }
    assert_eq!(f.worker.state.agent(f.agent).unwrap().current_request, None);
}

struct RejectRestartNotice {
    inner: Box<dyn Transport>,
    methods: Arc<Mutex<Vec<Method>>>,
}

impl Transport for RejectRestartNotice {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        if matches!(&method, Method::AgentPromptIfIdleAtGeneration(_)) {
            self.methods.lock().unwrap().push(method);
            return Err(TransportError {
                timestamps: Default::default(),
                code: Some(crate::protocol::api::AGENT_PROMPT_NOT_SHOWN.into()),
                message: crate::protocol::api::AGENT_PROMPT_NOT_SHOWN_MESSAGE.into(),
                definitely_rejected: true,
            });
        }
        self.inner.request(method)
    }
}

#[test]
fn restart_epoch_refused_notice_never_replays_task_and_times_out_its_group() {
    let mut f = Fixture::new(Provider::Codex);
    observe_epoch(&mut f, 1);
    let lead = running(&mut f, "PRIVATE_ORIGINAL_TASK", "turn-1");
    let member = f.send("PRIVATE_ORIGINAL_FOLLOWUP", false);
    f.deliver();
    f.started("turn-1", "PRIVATE_ORIGINAL_FOLLOWUP");
    f.consume();
    let queued = f.send("later independent task", true);
    epoch_reopen(&mut f, true, 2);
    f.worker.transport = Box::new(RejectRestartNotice {
        inner: epoch_transport(&f, true, 2),
        methods: f.native.clone(),
    });
    let before = f.typed().len();
    f.worker.tick().unwrap();
    assert!(!f.request(lead).settled(), "{:?}", f.request(lead));
    f.worker.withdrawn_at.clear();
    f.worker.tick().unwrap();
    for (text, _) in &f.typed()[before..] {
        assert!(text.contains("Bus restarted"), "{text}");
        assert!(!text.contains("PRIVATE_ORIGINAL"), "{text}");
        assert!(!text.contains("later independent task"), "{text}");
    }
    f.expire_after_reconciliation(io::now_ms() + 2 * 60 * 1000 + 1);
    for id in [lead, member] {
        assert_eq!(f.request(id).phase, RequestPhase::Abandoned);
        assert_eq!(
            f.request(id).failure_reason.as_deref(),
            Some("reply_not_captured")
        );
    }
    assert_eq!(f.request(queued).phase, RequestPhase::Queued);
    assert_eq!(f.worker.state.agent(f.agent).unwrap().current_request, None);
}

#[test]
fn restart_epoch_reopening_continuation_does_not_duplicate_the_notice() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    observe_epoch(&mut f, 1);
    let lead = running(&mut f, "original task", "turn-1");
    epoch_reopen(&mut f, true, 2);
    f.worker.tick().unwrap();
    assert!(!f.request(lead).settled(), "{:?}", f.request(lead));
    let notice = f.typed().last().unwrap().0.clone();
    let count = f.typed().len();
    epoch_reopen(&mut f, true, 2);
    f.worker.tick().unwrap();
    assert_eq!(f.typed().len(), count);
    f.started("turn-2", &notice);
    f.consume();
    f.stopped("turn-2", "complete after coordinator reopen");
    f.consume();
    assert_completed_without_reply(&f, lead);
}

#[test]
fn restart_epoch_second_restart_keeps_the_original_request_correlation() {
    let mut f = Fixture::new(Provider::Codex);
    observe_epoch(&mut f, 1);
    let lead = running(&mut f, "ORIGINAL_NEVER_REPLAY", "turn-1");
    epoch_reopen(&mut f, true, 2);
    f.worker.tick().unwrap();
    assert!(!f.request(lead).settled(), "{:?}", f.request(lead));
    let notice = f.typed().last().unwrap().0.clone();
    f.started("turn-2", &notice);
    f.consume();
    epoch_reopen(&mut f, true, 3);
    f.worker.tick().unwrap();
    assert!(!f.request(lead).settled(), "{:?}", f.request(lead));
    let notice = f.typed().last().unwrap().0.clone();
    assert!(!notice.contains("ORIGINAL_NEVER_REPLAY"));
    f.started("turn-3", &notice);
    f.consume();
    f.stopped("turn-3", "complete after two restarts");
    f.consume();
    assert_completed_without_reply(&f, lead);
    assert_eq!(f.worker.state.requests().count(), 1);
}

#[test]
fn restart_epoch_failed_cold_resume_closes_preserved_original_group() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    observe_epoch(&mut f, 1);
    let lead = running(&mut f, "original task", "turn-1");
    let member = f.send("joined followup", false);
    f.deliver();
    f.started("turn-1", "joined followup");
    f.consume();
    epoch_reopen(&mut f, true, 2);
    f.worker.transport = Box::new(RejectRestartNotice {
        inner: epoch_transport(&f, true, 2),
        methods: f.native.clone(),
    });
    f.worker.tick().unwrap();
    assert!(!f.request(lead).settled(), "{:?}", f.request(lead));
    let mut state = f.worker.state.clone();
    state
        .set_agent_startup(
            f.agent,
            Some(crate::messaging::model::AgentStartup {
                command: "claude --resume saved".into(),
                started_at_ms: io::now_ms(),
                reported: true,
                process_observed: false,
                native_terminal_id: Some("restored-terminal".into()),
                recovery: None,
                failure: Some(crate::messaging::model::StartupFailure {
                    reason: "saved conversation is unavailable".into(),
                    recovery: "replace the agent".into(),
                }),
            }),
        )
        .unwrap();
    state.abandon_failed_startup_queue(f.agent, io::now_ms());
    f.worker.save(state).unwrap();
    for id in [lead, member] {
        assert_eq!(f.request(id).phase, RequestPhase::Abandoned);
        assert_eq!(
            f.request(id).failure_reason.as_deref(),
            Some("reply_not_captured")
        );
        assert!(f
            .request(id)
            .submission_error
            .as_deref()
            .unwrap()
            .contains("saved conversation is unavailable"));
    }
    assert_eq!(f.worker.state.agent(f.agent).unwrap().current_request, None);
    assert!(f.worker.state.queued_requests(f.agent).is_empty());
}

#[test]
fn restart_epoch_keeps_a_captured_lead_reply_while_resuming_its_unanswered_member() {
    let mut f = Fixture::new(Provider::Codex);
    observe_epoch(&mut f, 1);
    let lead = running(&mut f, "original task", "turn-1");
    let member = f.send("joined followup", false);
    f.deliver();
    f.started("turn-1", "joined followup");
    f.consume();
    let mut state = f.worker.state.clone();
    state
        .answer_message(
            f.request(lead).prompt.id,
            f.agent,
            "captured before restart".into(),
            io::now_ms(),
        )
        .unwrap();
    f.worker.save(state).unwrap();
    let captured = f.request(lead).completed_reply.clone();
    epoch_reopen(&mut f, true, 2);
    f.worker.tick().unwrap();
    assert_eq!(f.request(lead).completed_reply, captured);
    assert!(!f.request(member).settled(), "{:?}", f.request(member));
    assert_eq!(
        f.worker.state.agent(f.agent).unwrap().current_request,
        Some(member)
    );
    let notice = f.typed().last().unwrap().0.clone();
    assert!(notice.contains(&format!("message {}", f.request(member).prompt.id.0)));
    f.started("turn-2", &notice);
    f.consume();
    f.stopped("turn-2", "remaining work completed");
    f.consume();
    assert_eq!(f.request(lead).completed_reply, captured);
    assert_completed_without_reply(&f, member);
}

#[test]
fn restart_epoch_invalidated_resume_closes_the_original_request() {
    let mut f = Fixture::new(Provider::Codex);
    observe_epoch(&mut f, 1);
    let lead = running(&mut f, "task whose saved session cannot resume", "turn-1");
    epoch_reopen(&mut f, true, 2);
    let mut state = f.worker.state.clone();
    state.invalidate_agent_session(f.agent).unwrap();
    f.worker.save(state).unwrap();
    let before = f.typed().len();
    f.worker.tick().unwrap();
    assert_eq!(f.request(lead).phase, RequestPhase::Abandoned);
    assert_eq!(
        f.request(lead).failure_reason.as_deref(),
        Some("reply_not_captured")
    );
    assert_eq!(
        f.typed().len(),
        before,
        "an invalidated conversation must not receive a notice"
    );
}

#[test]
fn restart_epoch_provider_retry_keeps_the_resumed_group_and_notice_only_payload() {
    let mut f = Fixture::new(Provider::Codex);
    observe_epoch(&mut f, 1);
    let lead = running(&mut f, "ORIGINAL_TASK_WITH_SIDE_EFFECTS", "turn-1");
    let member = f.send("ORIGINAL_JOINED_TASK", false);
    f.deliver();
    f.started("turn-1", "ORIGINAL_JOINED_TASK");
    f.consume();
    epoch_reopen(&mut f, true, 2);
    f.worker.tick().unwrap();
    assert!(!f.request(lead).settled(), "{:?}", f.request(lead));
    let notice = f.typed().last().unwrap().0.clone();
    f.started("turn-2", &notice);
    f.consume();
    // Model a due provider retry after the resumed turn was bound. Keep the
    // real retry preparation, grouping, submission and callback code in use.
    let mut document = serde_json::to_value(&f.worker.state).unwrap();
    document["agents"][f.agent.0.to_string()]["provider_retry"] = json!({
        "lead": lead, "attempts": 1, "retry_at_ms": 0, "quota": false, "queued": false,
    });
    f.worker
        .save(serde_json::from_value(document).unwrap())
        .unwrap();
    f.status(RuntimeStatus::Idle);
    let before = f.typed().len();
    f.deliver();
    assert_eq!(f.typed().len(), before + 1);
    assert_eq!(f.request(member).group, Some(lead));
    let retry_notice = f.typed().last().unwrap().0.clone();
    assert!(!retry_notice.contains("ORIGINAL_TASK_WITH_SIDE_EFFECTS"));
    assert!(!retry_notice.contains("ORIGINAL_JOINED_TASK"));
    f.started("turn-3", &retry_notice);
    f.consume();
    f.stopped("turn-3", "resumed group completed after retry");
    f.consume();
    assert_completed_without_reply(&f, lead);
    assert_completed_without_reply(&f, member);
}

#[test]
fn restart_epoch_deleting_resumed_recipient_closes_originals_and_queued_work() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    observe_epoch(&mut f, 1);
    let lead = running(&mut f, "interrupted original", "turn-1");
    let member = f.send("interrupted joined input", false);
    f.deliver();
    f.started("turn-1", "interrupted joined input");
    f.consume();
    let queued = f.send("independent queued input", true);
    epoch_reopen(&mut f, true, 2);
    f.worker.tick().unwrap();
    assert!(!f.request(lead).settled());
    let before = f.typed().len();
    let mut state = f.worker.state.clone();
    state.delete_agent(f.agent, io::now_ms()).unwrap();
    f.worker.save(state).unwrap();
    f.worker.submit_ready().unwrap();
    assert!(f.worker.state.agent(f.agent).is_none());
    for id in [lead, member, queued] {
        assert_eq!(f.request(id).phase, RequestPhase::Abandoned);
        assert_eq!(
            f.request(id).failure_reason.as_deref(),
            Some("agent_deleted")
        );
        assert!(f.reply(id).is_none());
    }
    assert_eq!(f.typed().len(), before, "deleted work must never replay");
}

#[test]
fn restart_epoch_resumed_turn_accepts_new_steering_normally() {
    for provider in [Provider::Codex, Provider::ClaudeCode] {
        let mut f = Fixture::new(provider);
        observe_epoch(&mut f, 1);
        let lead = running(&mut f, "original task", "turn-1");
        epoch_reopen(&mut f, true, 2);
        f.worker.tick().unwrap();
        assert!(!f.request(lead).settled());
        let notice = f.typed().last().unwrap().0.clone();
        f.started("turn-2", &notice);
        f.consume();
        f.status(RuntimeStatus::Working);
        let member = f.send("new correction after resume", false);
        let before = f.typed().len();
        f.deliver();
        assert_eq!(f.typed().len(), before + 1, "{provider:?}");
        assert_eq!(f.request(member).group, Some(lead));
        f.started("turn-2", "new correction after resume");
        f.consume();
        f.stopped("turn-2", "finished resumed task and correction");
        f.consume();
        assert_completed_without_reply(&f, lead);
        assert_completed_without_reply(&f, member);
    }
}

#[test]
fn restart_epoch_second_restart_before_notice_delivery_preserves_group() {
    for provider in [Provider::Codex, Provider::ClaudeCode] {
        let mut f = Fixture::new(provider);
        observe_epoch(&mut f, 1);
        let lead = running(&mut f, "original interrupted task", "turn-1");
        let member = f.send("joined interrupted input", false);
        f.deliver();
        f.started("turn-1", "joined interrupted input");
        f.consume();
        let queued = f.send("later independent task", true);

        epoch_reopen(&mut f, true, 2);
        f.worker.transport = Box::new(RejectRestartNotice {
            inner: epoch_transport(&f, true, 2),
            methods: f.native.clone(),
        });
        f.worker.tick().unwrap();
        assert_eq!(f.request(lead).phase, RequestPhase::Queued);
        assert_eq!(f.request(member).group, Some(lead));

        epoch_reopen(&mut f, true, 3);
        f.worker.tick().unwrap();
        assert_eq!(
            f.request(member).group,
            Some(lead),
            "{provider:?}: another restart before notice delivery must retain the original group"
        );
        assert_eq!(f.request(queued).phase, RequestPhase::Queued);
        let notice = f.typed().last().unwrap().0.clone();
        f.started("turn-3", &notice);
        f.consume();
        f.stopped("turn-3", "finished the interrupted group");
        f.consume();
        assert_completed_without_reply(&f, lead);
        assert_completed_without_reply(&f, member);
        f.status(RuntimeStatus::Idle);
        f.deliver();
        assert_eq!(
            f.worker.state.agent(f.agent).unwrap().current_request,
            Some(queued),
            "there must be no second continuation for the original group member"
        );
    }
}
