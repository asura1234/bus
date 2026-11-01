use super::*;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};

fn home(fixture: &Fixture) -> PathBuf {
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

fn native_turn(turn: u64, prompt: &str, guidance: &[String], final_text: &str) -> String {
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
        assert_eq!(
            fixture.reply(lead).as_deref(),
            Some("batched guidance answer"),
            "case {case}: lead"
        );
        for id in [first, second] {
            assert_eq!(
                fixture.reply(id).as_deref(),
                owned.then_some("batched guidance answer"),
                "case {case}: request {id:?}"
            );
        }
    }
}

fn stop(fixture: &Fixture) {
    fixture.record(json!({"hook_event_name":"Stop","session_id":"session"}));
}

fn reopen(fixture: &mut Fixture) {
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
    for id in [lead, first, second] {
        assert_eq!(
            fixture.reply(id).as_deref(),
            Some("ordered guidance answer"),
            "request {id:?}"
        );
        assert!(fixture.worker.state.turn_ended(fixture.request(id)));
    }
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
    assert_eq!(fixture.reply(lead).as_deref(), Some("old final"));
    assert_eq!(fixture.reply(next).as_deref(), Some("next final"));
    assert_eq!(fixture.request(lead).provider_turn_id.as_deref(), Some("1"));
    assert_eq!(fixture.request(next).provider_turn_id.as_deref(), Some("2"));
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
    assert!(fixture.reply(lead).is_some());
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
        assert_eq!(fixture.reply(request).as_deref(), Some(task));
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
    assert_eq!(
        fixture.reply(lead).as_deref(),
        Some("old final with guidance")
    );
    assert_eq!(
        fixture.reply(guidance).as_deref(),
        Some("old final with guidance")
    );
    assert_eq!(fixture.reply(next).as_deref(), Some("next final"));
    assert_eq!(
        fixture.request(guidance).provider_turn_id.as_deref(),
        Some("1")
    );
    assert_eq!(fixture.request(next).provider_turn_id.as_deref(), Some("2"));
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
    assert_eq!(fixture.reply(lead).as_deref(), Some("old final"));
    std::fs::write(
        &wire,
        format!("{}{}", old, native_turn(2, &typed[1].0, &[], "next final")),
    )
    .unwrap();
    fixture.started("2", "native queued follow-up");
    stop(&fixture);
    fixture.consume();
    assert_eq!(
        fixture.reply(next).as_deref(),
        Some("next final"),
        "a poll before the queued native turn starts must not lose its request"
    );
    assert_eq!(fixture.request(next).provider_turn_id.as_deref(), Some("2"));
}

#[path = "kimi_boundary_test.rs"]
mod boundary_tests;
