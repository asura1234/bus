use super::*;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};

pub(super) fn home(fixture: &Fixture) -> PathBuf {
    let spool = fixture.dir.join("callbacks/launch");
    let home = spool.join("kimi-home");
    let source = fixture.dir.join("synthetic-auth");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
    for folder in ["credentials", "oauth"] {
        std::fs::create_dir_all(source.join(folder)).unwrap();
        symlink(source.join(folder), home.join(folder)).unwrap();
    }
    let metadata = home.metadata().unwrap();
    std::fs::write(
        spool.join("kimi-home.json"),
        serde_json::to_vec(
            &json!({"path":home,"source":source,"device":metadata.dev(),"inode":metadata.ino()}),
        )
        .unwrap(),
    )
    .unwrap();
    let session = home.join("sessions/workspace/session");
    std::fs::create_dir_all(session.join("agents/main")).unwrap();
    std::fs::write(
        home.join("session_index.jsonl"),
        format!(
            "{}\n",
            json!({"sessionId":"session","workDir":fixture.dir,"sessionDir":session})
        ),
    )
    .unwrap();
    session.join("agents/main/wire.jsonl")
}

pub(super) fn native_turn(
    turn: u64,
    prompt: &str,
    guidance: &[String],
    final_text: &str,
) -> String {
    let prompt_id = format!("p{turn}");
    let mut rows = vec![
        json!({"type":"turn.prompt","agentId":"main","turnId":turn,"promptId":prompt_id,"input":[{"type":"text","text":prompt}]}),
    ];
    for (n, text) in guidance.iter().enumerate() {
        rows.push(json!({"type":"prompt.steered","agentId":"main","activePromptId":prompt_id,"promptIds":[format!("g{turn}-{n}")],"messageId":format!("g{turn}-{n}"),"content":[{"type":"text","text":text}]}));
    }
    rows.extend([
        json!({"type":"agent.message.appended","message":{"meta":{"source":"llm"},"message":{"role":"assistant","content":[{"type":"text","text":final_text}]}}}),
        json!({"type":"agent.turn.ended","turnId":turn,"outcome":"done"}),
        json!({"type":"turn.ended","agentId":"main","turnId":turn,"reason":"completed"}),
        json!({"type":"prompt.completed","agentId":"main","promptId":prompt_id,"reason":"completed"}),
    ]);
    rows.into_iter().map(|row| format!("{row}\n")).collect()
}

fn settled_without_reply(fixture: &Fixture, id: RequestId) {
    assert_eq!(fixture.request(id).phase, RequestPhase::Completed, "{id:?}");
    assert!(
        fixture.request(id).failure_reason.is_none(),
        "{id:?} {:?}",
        fixture.request(id).failure_reason
    );
    assert!(fixture.reply(id).is_none(), "{id:?}");
}

fn final_text_absent(fixture: &Fixture, text: &str) {
    if text.is_empty() {
        return;
    }
    assert!(
        fixture
            .worker
            .state
            .requests()
            .all(|request| { request.reply().is_none_or(|reply| reply.text != text) }),
        "{text} stored on a request"
    );
    assert!(
        fixture
            .worker
            .state
            .room(fixture.room)
            .unwrap()
            .latest_replies
            .values()
            .all(|reply| reply.text != text),
        "{text} published"
    );
}

fn native_batched_guidance_turn(
    turn: u64,
    prompt: &str,
    guidance: &[String],
    prompt_ids: &[String],
    active_prompt: &str,
    final_text: &str,
) -> String {
    let batch = json!({"type":"prompt.steered","agentId":"main","activePromptId":active_prompt,"promptIds":prompt_ids,"messageId":"batch","content":guidance.iter().map(|text| json!({"type":"text","text":text})).collect::<Vec<_>>()});
    native_turn(turn, prompt, &[], final_text).replacen('\n', &format!("\n{batch}\n"), 1)
}

#[test]
fn kimi_batched_guidance_requires_attested_part_and_prompt_boundaries() {
    for case in 0..4 {
        let mut fixture = Fixture::new(Provider::Kimi);
        let wire = home(&fixture);
        let lead = running(&mut fixture, "original task", "1");
        let first = fixture.send("first correction", false);
        let second = fixture.send("second correction", false);
        fixture.deliver();
        fixture.deliver();
        let typed = fixture.typed();
        let separate = vec![typed[1].0.clone(), typed[2].0.clone()];
        let concatenated = vec![format!("{}{}", typed[1].0, typed[2].0)];
        let ids = vec!["g1".to_owned(), "g2".to_owned()];
        let (content, prompt_ids, active, owned) = match case {
            0 => (concatenated, ids.clone(), "p1", false),
            1 => (separate.clone(), ids[..1].to_vec(), "p1", false),
            2 => (separate.clone(), ids.clone(), "other", false),
            _ => (separate.clone(), ids.clone(), "p1", true),
        };
        let original = fixture.worker.state.render_request_payload(lead).unwrap();
        std::fs::write(
            &wire,
            native_batched_guidance_turn(
                1,
                &original,
                &content,
                &prompt_ids,
                active,
                "batched guidance answer",
            ),
        )
        .unwrap();
        fixture.status(RuntimeStatus::Idle);
        stop(&fixture);
        fixture.consume();
        settled_without_reply(&fixture, lead);
        final_text_absent(&fixture, "batched guidance answer");
        assert_eq!(
            fixture.request(lead).provider_turn_id.as_deref(),
            Some("1"),
            "case {case}: lead"
        );
        for id in [first, second] {
            assert!(fixture.reply(id).is_none(), "case {case}: request {id:?}");
            if owned {
                settled_without_reply(&fixture, id);
                assert_eq!(fixture.request(id).group, Some(lead));
                assert_eq!(fixture.request(id).provider_turn_id.as_deref(), Some("1"));
            } else {
                assert!(
                    !fixture.request(id).settled(),
                    "case {case}: unowned {id:?} completed as if it shared the final"
                );
                assert_ne!(
                    fixture.request(id).failure_reason.as_deref(),
                    Some("reply_not_captured")
                );
            }
        }
    }
}

fn stop(fixture: &Fixture) {
    fixture.record(json!({"hook_event_name":"Stop","session_id":"session"}));
}

pub(super) fn reopen(fixture: &mut Fixture) {
    let replacement = Worker::open(
        crate::utils::test_temp::unique_temp_path("kimi-restart-swap"),
        Box::new(Native(fixture.native.clone())),
    )
    .unwrap();
    let swap = replacement.data_dir.clone();
    drop(std::mem::replace(&mut fixture.worker, replacement));
    fixture.worker = Worker::open(
        fixture.dir.clone(),
        Box::new(Native(fixture.native.clone())),
    )
    .unwrap();
    std::fs::remove_dir_all(swap).unwrap();
}

#[test]
fn kimi_multiple_busy_messages_share_only_the_committed_guidance_turn() {
    guidance_case(false, false);
}

#[test]
fn kimi_steering_after_restart_preserves_order_and_turn_ownership() {
    guidance_case(true, false);
}

#[test]
fn kimi_consumed_guidance_with_lost_acknowledgments_settles_without_retry() {
    guidance_case(false, true);
}

struct LostAcknowledgment(Arc<Mutex<Vec<Method>>>);

impl Transport for LostAcknowledgment {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        Native(self.0.clone()).request(method)?;
        Ok(ResponseResult::Ok {})
    }
}

fn guidance_case(restart: bool, uncertain: bool) {
    let mut fixture = Fixture::new(Provider::Kimi);
    let wire = home(&fixture);
    let lead = running(&mut fixture, "original task", "1");
    if restart {
        reopen(&mut fixture);
        fixture.status(RuntimeStatus::Working);
    }
    if uncertain {
        fixture.worker.transport = Box::new(LostAcknowledgment(fixture.native.clone()));
    }
    let first = fixture.send("first correction", false);
    let second = fixture.send("second correction", false);
    fixture.deliver();
    if uncertain {
        assert_eq!(fixture.typed().len(), 2);
        assert_eq!(fixture.request(second).phase, RequestPhase::Queued);
        fixture.started("1", "first correction");
        fixture.consume();
    }
    fixture.deliver();
    let typed = fixture.typed();
    assert_eq!(
        typed.len(),
        3,
        "agent={:?}; lead={:?}",
        fixture.worker.state.agent(fixture.agent),
        fixture.request(lead)
    );
    assert!(typed[1].1 && typed[2].1);
    assert!(typed[1].0.contains("first correction"));
    assert!(typed[2].0.contains("second correction"));
    let original = fixture.worker.state.render_request_payload(lead).unwrap();
    std::fs::write(
        &wire,
        native_turn(
            1,
            &original,
            &[typed[1].0.clone(), typed[2].0.clone()],
            "ordered guidance answer",
        ),
    )
    .unwrap();
    fixture.status(RuntimeStatus::Idle);
    stop(&fixture);
    fixture.consume();
    final_text_absent(&fixture, "ordered guidance answer");
    for id in [lead, first, second] {
        settled_without_reply(&fixture, id);
        assert_eq!(fixture.request(id).provider_turn_id.as_deref(), Some("1"));
        assert!(fixture.worker.state.turn_ended(fixture.request(id)));
    }
    assert_eq!(fixture.request(first).group, Some(lead));
    assert_eq!(fixture.request(second).group, Some(lead));
    assert_eq!(
        fixture
            .worker
            .state
            .agent(fixture.agent)
            .unwrap()
            .current_request,
        None
    );
    fixture.deliver();
    assert_eq!(
        fixture.typed().len(),
        3,
        "no duplicate after native consumption"
    );
}

#[test]
fn kimi_old_final_is_reconciled_before_a_queued_native_turn_rebinds_the_lead() {
    queued_native_turn(true);
}

#[test]
fn kimi_committed_old_final_survives_a_missing_stop_before_the_next_started_hook() {
    queued_native_turn(false);
}

fn queued_native_turn(stop_before_start: bool) {
    let mut fixture = Fixture::new(Provider::Kimi);
    let wire = home(&fixture);
    let lead = running(&mut fixture, "old task", "1");
    let next = fixture.send("native queued follow-up", false);
    fixture.deliver();
    let typed = fixture.typed();
    // Both terminal chains exist before one callback batch, as in the live incident.
    std::fs::write(
        &wire,
        format!(
            "{}{}",
            native_turn(1, &typed[0].0, &[], "old final"),
            native_turn(2, &typed[1].0, &[], "next final")
        ),
    )
    .unwrap();
    if stop_before_start {
        stop(&fixture);
    }
    fixture.started("2", "native queued follow-up");
    stop(&fixture);
    fixture.status(RuntimeStatus::Working);
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    settled_without_reply(&fixture, lead);
    settled_without_reply(&fixture, next);
    final_text_absent(&fixture, "old final");
    final_text_absent(&fixture, "next final");
    assert_eq!(fixture.request(lead).provider_turn_id.as_deref(), Some("1"));
    assert_eq!(fixture.request(next).provider_turn_id.as_deref(), Some("2"));
    assert_eq!(fixture.request(next).group, None);
    assert_eq!(
        fixture
            .worker
            .state
            .agent(fixture.agent)
            .unwrap()
            .current_request,
        None
    );
    let fresh = fixture.send("fresh idle task", false);
    fixture.deliver();
    assert!(matches!(fixture.request(fresh).phase, RequestPhase::Active));
    assert!(!fixture.typed().last().unwrap().1);
}

#[test]
fn kimi_queue_only_messages_keep_independent_order_after_a_busy_turn() {
    let mut fixture = Fixture::new(Provider::Kimi);
    let wire = home(&fixture);
    let lead = running(&mut fixture, "busy task", "1");
    let first = fixture.send("first queued", true);
    let second = fixture.send("second queued", true);
    fixture.deliver();
    assert_eq!(fixture.typed().len(), 1);
    let mut transcript = native_turn(1, &fixture.typed()[0].0, &[], "lead final");
    std::fs::write(&wire, &transcript).unwrap();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    settled_without_reply(&fixture, lead);
    assert_eq!(fixture.request(lead).provider_turn_id.as_deref(), Some("1"));
    final_text_absent(&fixture, "lead final");
    for (turn, request, task) in [(2, first, "first queued"), (3, second, "second queued")] {
        fixture.deliver();
        assert_eq!(
            fixture.typed_content().last().unwrap(),
            &(task.to_owned(), false)
        );
        fixture.started(&turn.to_string(), task);
        fixture.consume();
        transcript.push_str(&native_turn(
            turn,
            &fixture.typed().last().unwrap().0,
            &[],
            task,
        ));
        std::fs::write(&wire, &transcript).unwrap();
        fixture.consume();
        settled_without_reply(&fixture, request);
        let turn_id = turn.to_string();
        assert_eq!(
            fixture.request(request).provider_turn_id.as_deref(),
            Some(turn_id.as_str())
        );
    }
    assert_eq!(fixture.typed().len(), 3);
}

#[test]
fn kimi_mixed_guidance_and_queued_turn_keep_their_own_committed_replies() {
    let mut fixture = Fixture::new(Provider::Kimi);
    let wire = home(&fixture);
    let lead = running(&mut fixture, "old task", "1");
    let guidance = fixture.send("guidance for old turn", false);
    fixture.deliver();
    let next = fixture.send("native queued follow-up", false);
    fixture.deliver();
    let typed = fixture.typed();
    std::fs::write(
        &wire,
        format!(
            "{}{}",
            native_turn(
                1,
                &typed[0].0,
                &[typed[1].0.clone()],
                "old final with guidance"
            ),
            native_turn(2, &typed[2].0, &[], "next final"),
        ),
    )
    .unwrap();
    stop(&fixture);
    fixture.started("2", "native queued follow-up");
    stop(&fixture);
    fixture.status(RuntimeStatus::Working);
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    settled_without_reply(&fixture, lead);
    settled_without_reply(&fixture, guidance);
    settled_without_reply(&fixture, next);
    final_text_absent(&fixture, "old final with guidance");
    final_text_absent(&fixture, "next final");
    assert_eq!(fixture.request(lead).provider_turn_id.as_deref(), Some("1"));
    assert_eq!(
        fixture.request(guidance).provider_turn_id.as_deref(),
        Some("1")
    );
    assert_eq!(fixture.request(guidance).group, Some(lead));
    assert_eq!(fixture.request(next).provider_turn_id.as_deref(), Some("2"));
    assert_eq!(fixture.request(next).group, None);
}

#[test]
fn kimi_queued_native_turn_can_start_after_an_old_final_poll() {
    let mut fixture = Fixture::new(Provider::Kimi);
    let wire = home(&fixture);
    let lead = running(&mut fixture, "old task", "1");
    let next = fixture.send("native queued follow-up", false);
    fixture.deliver();
    let typed = fixture.typed();
    let old = native_turn(1, &typed[0].0, &[], "old final");
    std::fs::write(&wire, &old).unwrap();
    stop(&fixture);
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    settled_without_reply(&fixture, lead);
    assert_eq!(fixture.request(lead).provider_turn_id.as_deref(), Some("1"));
    final_text_absent(&fixture, "old final");
    assert!(!fixture.request(next).settled());
    std::fs::write(
        &wire,
        format!("{}{}", old, native_turn(2, &typed[1].0, &[], "next final")),
    )
    .unwrap();
    fixture.started("2", "native queued follow-up");
    stop(&fixture);
    fixture.consume();
    settled_without_reply(&fixture, next);
    final_text_absent(&fixture, "next final");
    assert_eq!(
        fixture.request(next).provider_turn_id.as_deref(),
        Some("2"),
        "a poll before the queued native turn starts must not lose its request"
    );
    settled_without_reply(&fixture, lead);
    assert_eq!(fixture.request(lead).provider_turn_id.as_deref(), Some("1"));
}

#[path = "kimi_boundary_test.rs"]
mod boundary_tests;

#[path = "kimi_incident_replay_test.rs"]
mod incident_replays;

#[test]
fn orchestrator_independent_kimi_requests_wait_for_separate_committed_turns() {
    for restart in [false, true] {
        let mut f = Fixture::in_room(Provider::Kimi, true);
        let mut state = f.worker.state.clone();
        let work = state.create_room("work").unwrap();
        state.bind_orchestrator(f.agent, work).unwrap();
        let sender = state
            .create_agent(work, "worker", Provider::Codex, f.dir.clone(), None)
            .unwrap();
        f.worker.save(state).unwrap();
        f.room = work;
        let wire = home(&f);
        let lead = running(&mut f, "old question", "1");
        let mut state = f.worker.state.clone();
        let queued = state
            .submit_message_with(
                work,
                Draft {
                    text: "worker question".into(),
                    files: vec![],
                    recipient_ids: [f.agent].into(),
                },
                Author::Agent(sender),
                f.now,
                false,
            )
            .unwrap()[0];
        f.worker.save(state).unwrap();
        f.deliver();
        assert_eq!(
            f.typed().len(),
            1,
            "must not issue native guidance for independent work"
        );
        if restart {
            reopen(&mut f);
        }
        let first = native_turn(1, &f.typed()[0].0, &[], "old answer");
        std::fs::write(&wire, &first).unwrap();
        f.status(RuntimeStatus::Idle);
        stop(&f);
        f.consume();
        assert!(f.reply(lead).is_none());
        assert_eq!(f.request(lead).delivery_stage(), "ended");
        assert_eq!(f.reply(queued), None);
        assert_eq!(f.request(queued).group, None);
        f.deliver();
        assert_eq!(f.typed().len(), 2);
        assert!(!f.typed()[1].1);
        let payload = f.typed()[1].0.clone();
        f.started("2", &payload);
        f.consume();
        std::fs::write(
            &wire,
            format!("{first}{}", native_turn(2, &payload, &[], "worker answer")),
        )
        .unwrap();
        stop(&f);
        f.status(RuntimeStatus::Idle);
        f.consume();
        assert!(f.reply(queued).is_none());
        assert_eq!(f.request(queued).delivery_stage(), "ended");
        assert!(f.reply(lead).is_none());
        assert_eq!(f.request(lead).delivery_stage(), "ended");
        assert_eq!(f.request(queued).provider_turn_id.as_deref(), Some("2"));
    }
}
