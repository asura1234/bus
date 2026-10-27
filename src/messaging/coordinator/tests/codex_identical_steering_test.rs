use super::*;

#[test]
fn identical_steered_input_in_a_later_turn_does_not_take_the_lead_turn_answer() {
    for follow_text in ["check again", "status?"] {
        let (mut fixture, rollout) = with_session_rollout();
        let lead = fixture.send("status?", false);
        fixture.deliver();
        fixture.record(
            json!({"hook_event_name":"UserPromptSubmit","session_id":"session",
            "turn_id":"turn-1","prompt":"status?","transcript_path":rollout}),
        );
        fixture.consume();
        fixture.status(RuntimeStatus::Working);
        let follow = fixture.send(follow_text, false);
        fixture.deliver();
        assert_eq!(fixture.request(follow).group, Some(lead));
        assert!(!fixture.request(follow).trusted_start_bound);
        let typed = fixture.typed().last().unwrap().0.clone();
        append_lines(
            &rollout,
            &(recorded_turn(fixture.now + 1, "turn-1", "status?", "FIRST STATUS")
                + &recorded_turn(fixture.now + 2, "turn-2", &typed, "SECOND STATUS")),
        );
        idle_past_settle(&mut fixture);
        fixture.now += 5_000;
        fixture.status(RuntimeStatus::Idle);
        fixture.consume();
        assert_eq!(fixture.reply(lead).as_deref(), Some("FIRST STATUS"));
        assert_ne!(
            fixture.reply(follow).as_deref(),
            Some("FIRST STATUS"),
            "follow-up {follow_text:?} ran as turn-2, so turn-1's original prompt is not evidence it shares turn-1's answer"
        );
        assert_eq!(fixture.request(follow).phase, RequestPhase::Abandoned);
        assert_eq!(
            fixture.request(follow).failure_reason.as_deref(),
            Some("reply_not_captured")
        );
    }
}

/// An identical follow-up Codex took into the running turn is recorded there
/// a second time, once per submission, and shares that turn's answer.
#[test]
fn identical_steered_input_recorded_twice_in_the_lead_turn_shares_its_answer() {
    let (mut fixture, rollout) = with_session_rollout();
    let lead = fixture.send("status?", false);
    fixture.deliver();
    fixture.record(
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session",
        "turn_id":"turn-1","prompt":"status?","transcript_path":rollout}),
    );
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    let follow = fixture.send("status?", false);
    fixture.deliver();
    assert_eq!(fixture.request(follow).group, Some(lead));
    let typed = fixture.typed().last().unwrap().0.clone();
    let at = stamp(fixture.now + 1);
    let again = json!({"timestamp":at,"type":"event_msg","payload":{"type":"item_completed",
        "turn_id":"turn-1","item":{"type":"UserMessage","content":[{"type":"text","text":typed}]}}});
    let turn = recorded_turn(fixture.now + 1, "turn-1", "status?", "STATUS");
    let (head, complete) = turn.trim_end().rsplit_once('\n').unwrap();
    append_lines(&rollout, &format!("{head}\n{again}\n{complete}\n"));
    idle_past_settle(&mut fixture);
    fixture.now += 5_000;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    for id in [lead, follow] {
        assert_eq!(fixture.reply(id).as_deref(), Some("STATUS"));
    }
}
