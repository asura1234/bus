//! Scenarios retained from PR #256, using native guidance ownership.
use super::*;

#[test]
fn kimi_delayed_follow_up_keeps_nested_guidance_and_its_own_final() {
    for restart in [false, true] {
        let mut f = Fixture::new(Provider::Kimi);
        let wire = home(&f);
        let lead = running(&mut f, "lead task", "1");
        let follow = f.send("follow up", false);
        f.deliver();
        let typed = f.typed();
        let old = native_turn(1, &typed[0].0, &[], "lead final");
        std::fs::write(&wire, &old).unwrap();
        stop(&f);
        f.status(RuntimeStatus::Idle);
        f.consume();
        assert_eq!(f.reply(lead).as_deref(), Some("lead final"));
        assert!(
            !f.request(follow).settled(),
            "idle is not rejection of queued input"
        );
        if restart {
            reopen(&mut f);
        }
        let next = native_turn(2, &typed[1].0, &[], "follow final");
        let open = next.lines().next().unwrap();
        std::fs::write(&wire, format!("{old}{open}\n")).unwrap();
        f.status(RuntimeStatus::Working);
        f.started("2", "follow up");
        f.consume();
        assert_eq!(f.request(follow).provider_turn_id.as_deref(), Some("2"));
        assert_eq!(
            f.reply(follow),
            None,
            "an open follow-up cannot close early"
        );
        let nested = f.send("inside follow up", false);
        f.deliver();
        let nested_payload = f.typed().last().unwrap().0.clone();
        std::fs::write(
            &wire,
            format!(
                "{old}{}",
                native_turn(2, &typed[1].0, &[nested_payload], "follow final")
            ),
        )
        .unwrap();
        f.status(RuntimeStatus::Idle);
        stop(&f);
        f.consume();
        for id in [follow, nested] {
            assert_eq!(f.reply(id).as_deref(), Some("follow final"));
            assert_eq!(f.request(id).provider_turn_id.as_deref(), Some("2"));
        }
        assert_eq!(f.reply(lead).as_deref(), Some("lead final"));
        assert_eq!(f.worker.state.agent(f.agent).unwrap().current_request, None);
        let fresh = f.send("fresh task", false);
        f.deliver();
        assert_eq!(f.request(fresh).group, None);
        assert!(!f.typed().last().unwrap().1);
    }
}

#[test]
fn kimi_silent_synchronous_turn_and_guidance_do_not_strand_the_inbox() {
    for guidance in [false, true] {
        let mut f = Fixture::new(Provider::Kimi);
        let wire = home(&f);
        let lead = running(&mut f, "run pwd synchronously then finish silently", "1");
        let steer = guidance.then(|| {
            let id = f.send("reply STEER", false);
            f.deliver();
            id
        });
        let typed = f.typed();
        let turn = native_turn(
            1,
            &typed[0].0,
            &typed[1..].iter().map(|p| p.0.clone()).collect::<Vec<_>>(),
            if guidance { "STEER" } else { "" },
        );
        let tool = json!({"type":"agent.message.appended","message":{"meta":{"source":"llm"},"message":{"role":"assistant","content":[],"toolCalls":[{"id":"sync-pwd","name":"Bash","arguments":{"command":"pwd"}}]}}});
        let result = json!({"type":"agent.message.appended","message":{"meta":{"source":"tool"},"message":{"role":"tool","toolCallId":"sync-pwd","content":[{"type":"text","text":"/tmp/project\n"}]}}});
        std::fs::write(
            &wire,
            turn.replacen('\n', &format!("\n{tool}\n{result}\n"), 1),
        )
        .unwrap();
        f.status(RuntimeStatus::Idle);
        stop(&f);
        f.consume();
        assert!(f.request(lead).settled());
        assert_eq!(f.reply(lead).as_deref(), guidance.then_some("STEER"));
        if let Some(id) = steer {
            assert_eq!(f.reply(id).as_deref(), Some("STEER"));
        }
        assert_eq!(f.worker.state.agent(f.agent).unwrap().current_request, None);
        let fresh = f.send("next marker", false);
        f.deliver();
        assert_eq!(f.request(fresh).phase, RequestPhase::Active);
    }
}

#[test]
fn kimi_interrupt_control_sends_esc_once_and_settles_after_idle() {
    let mut f = Fixture::new(Provider::Kimi);
    home(&f);
    let lead = running(&mut f, "long task", "1");
    let queued = f.send("queued task", true);
    let response = f.worker.control_response_with_events(
        &crate::messaging::control::Request {
            id: "kimi-interrupt".into(),
            method: "agent.interrupt".into(),
            params: json!({"agent":"builder"}),
        },
        None,
    );
    assert!(response.ok, "{response:?}");
    let keys: Vec<_> = f
        .native
        .lock()
        .unwrap()
        .iter()
        .filter_map(|m| match m {
            Method::AgentSendKeys(p) => Some(p.keys.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(keys, [vec!["esc".to_owned()]]);
    assert!(!f.request(lead).settled());
    f.status(RuntimeStatus::Idle);
    for id in [lead, queued] {
        assert_eq!(f.request(id).failure_reason.as_deref(), Some("interrupted"));
    }
    let fresh = f.send("after interrupt", false);
    f.deliver();
    assert_eq!(f.request(fresh).group, None);
    assert!(!f.typed().last().unwrap().1);
}

#[test]
fn kimi_missing_stop_poll_retains_a_follow_up_after_answered_or_silent_completion() {
    for answer in ["old final", ""] {
        let mut f = Fixture::new(Provider::Kimi);
        let wire = home(&f);
        let lead = running(&mut f, "first task", "1");
        let follow = f.send("later task", false);
        f.deliver();
        let typed = f.typed();
        let old = native_turn(1, &typed[0].0, &[], answer);
        std::fs::write(&wire, &old).unwrap();
        f.status(RuntimeStatus::Idle);
        f.consume(); // no Stop: only the periodic wire read sees completion
        assert_eq!(f.request(lead).phase, RequestPhase::Completed);
        assert!(
            !f.request(follow).settled(),
            "{answer:?}: later turn has not started"
        );
        std::fs::write(
            &wire,
            format!("{old}{}", native_turn(2, &typed[1].0, &[], "later final")),
        )
        .unwrap();
        f.started("2", "later task");
        f.consume();
        f.worker.settle_requests().unwrap();
        assert_eq!(f.reply(follow).as_deref(), Some("later final"));
        assert_eq!(
            f.reply(lead).as_deref(),
            (!answer.is_empty()).then_some(answer)
        );
    }
}

#[test]
fn kimi_unclaimed_input_expires_without_replay_after_its_retention_window() {
    let mut f = Fixture::new(Provider::Kimi);
    let wire = home(&f);
    let lead = running(&mut f, "old task", "1");
    let follow = f.send("unclaimed input", false);
    f.deliver();
    std::fs::write(&wire, native_turn(1, &f.typed()[0].0, &[], "old final")).unwrap();
    stop(&f);
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert!(!f.request(follow).settled());
    assert_eq!(f.reply(lead).as_deref(), Some("old final"));
    f.now = f
        .request(lead)
        .completed_at_ms
        .unwrap()
        .max(f.worker.state.agent(f.agent).unwrap().status_since_ms)
        + crate::messaging::model::DELIVERED_STALL_MS
        + 1;
    f.status(RuntimeStatus::Idle);
    f.worker.settle_requests().unwrap();
    assert_eq!(
        f.request(follow).failure_reason.as_deref(),
        Some("submission_unconfirmed")
    );
    assert!(f.request(follow).uncertain_outcome);
    assert_eq!(
        f.typed().len(),
        2,
        "a possibly consumed input is never replayed"
    );
    assert_eq!(f.worker.state.agent(f.agent).unwrap().current_request, None);
}
