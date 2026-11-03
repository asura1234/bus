//! Replays minimized native records from temp/kimi-reliability/finding.md.
//! Frozen delivery bytes and native IDs are retained; unrelated tool traffic is omitted.
use super::*;

fn prior_active() -> Value {
    serde_json::from_str(include_str!(
        "fixtures/kimi_reliability/sequence-prior-active.json"
    ))
    .unwrap()
}

fn guidance_cases() -> Vec<Value> {
    serde_json::from_str(include_str!(
        "fixtures/kimi_reliability/sequence-guidance.json"
    ))
    .unwrap()
}

/// Keep recorded intervals while rebasing wall-clock timestamps onto this
/// fixture. submit_ready expires queues against the current clock, so the old
/// absolute incident times must remain provenance rather than runtime deadlines.
struct ReplayClock {
    source_origin_ms: u64,
    fixture_origin_ms: u64,
}

impl ReplayClock {
    fn new(f: &Fixture, lead: &Value) -> Self {
        Self {
            source_origin_ms: lead["submitted_at_ms"].as_u64().unwrap(),
            fixture_origin_ms: f.now,
        }
    }

    fn at(&self, source_ms: u64) -> u64 {
        self.fixture_origin_ms + source_ms.checked_sub(self.source_origin_ms).unwrap()
    }
}

/// Hydrate the saved delivery identity before native submission. Some recorded
/// messages have multiple recipients, so request_id need not be message_id + 1.
fn send_recorded(f: &mut Fixture, recorded: &Value, clock: &ReplayClock) -> RequestId {
    let request = recorded["request_id"].as_u64().unwrap();
    let mut document = serde_json::to_value(&f.worker.state).unwrap();
    document["next_id"] = json!(request - 1);
    f.worker
        .save(serde_json::from_value(document).unwrap())
        .unwrap();
    f.now = clock.at(recorded["submitted_at_ms"].as_u64().unwrap());
    let id = f.send(recorded["text"].as_str().unwrap(), false);
    assert_eq!(id, RequestId(request));
    let mut document = serde_json::to_value(&f.worker.state).unwrap();
    document["requests"][request.to_string()]["prompt"]["id"] = recorded["message_id"].clone();
    document["rooms"][f.room.0.to_string()]["latest_prompt"]["id"] = recorded["message_id"].clone();
    if recorded["payload"].is_string() {
        document["requests"][request.to_string()]["submitted_payload"] =
            recorded["payload"].clone();
    }
    f.worker
        .save(serde_json::from_value(document).unwrap())
        .unwrap();
    id
}

fn start_recorded(f: &mut Fixture, recorded: &Value, turn: &str, clock: &ReplayClock) -> RequestId {
    let lead = send_recorded(f, recorded, clock);
    f.deliver();
    assert_eq!(
        f.typed().len(),
        1,
        "recorded lead {lead:?} was not submitted: {:?}",
        f.request(lead)
    );
    assert_eq!(f.typed()[0].0, recorded["payload"].as_str().unwrap());
    f.started(turn, recorded["payload"].as_str().unwrap());
    f.consume();
    f.status(RuntimeStatus::Working);
    assert!(f.request(lead).trusted_start_bound);
    lead
}

fn wire_rows(rows: &[Value]) -> String {
    rows.iter().map(|row| format!("{row}\n")).collect()
}

#[test]
fn kimi_incident_35560_late_stop_cannot_rebind_the_completed_lead() {
    replay_prior_active(true);
}

#[test]
fn kimi_incident_35560_missing_stop_cannot_rebind_the_completed_lead() {
    replay_prior_active(false);
}

fn replay_prior_active(late_stop: bool) {
    let incident = prior_active();
    let turns = incident["turns"].as_array().unwrap();
    let mut f = Fixture::new(Provider::Kimi);
    let wire = home(&f);
    let clock = ReplayClock::new(&f, &incident["lead"]);
    let lead = start_recorded(&mut f, &incident["lead"], "1", &clock);
    let next = send_recorded(&mut f, &incident["follow"], &clock);
    f.deliver();
    assert_eq!(
        f.typed()[1].0,
        incident["follow"]["payload"].as_str().unwrap()
    );
    let old = wire_rows(turns[0]["rows"].as_array().unwrap());
    // Native turn 2 starts 26 ms after turn 1's committed completion. Stop is
    // delivered only after Started(2), or is absent altogether.
    std::fs::write(&wire, format!("{old}{}\n", turns[1]["rows"][0])).unwrap();
    f.now = clock.at(turns[1]["rows"][0]["time"].as_u64().unwrap());
    f.started("2", incident["follow"]["payload"].as_str().unwrap());
    if late_stop {
        stop(&f);
    }
    f.consume();
    assert_eq!(f.reply(lead).as_deref(), turns[0]["reply"].as_str());
    assert_eq!(f.request(lead).provider_turn_id.as_deref(), Some("1"));
    assert_eq!(f.request(next).provider_turn_id.as_deref(), Some("2"));
    assert!(
        f.reply(next).is_none(),
        "the queued native turn is still open"
    );

    let all = turns
        .iter()
        .map(|turn| wire_rows(turn["rows"].as_array().unwrap()))
        .collect::<String>();
    std::fs::write(&wire, all).unwrap();
    // These original manual prompts were the later historical 2 -> 3 -> 4
    // rebindings. Neither is the frozen Bus input for either request.
    for turn in &turns[2..] {
        f.now = clock.at(turn["rows"][0]["time"].as_u64().unwrap());
        f.started(
            &turn["turn_id"].to_string(),
            turn["payload"].as_str().unwrap(),
        );
    }
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_eq!(f.reply(lead).as_deref(), turns[0]["reply"].as_str());
    assert_eq!(f.reply(next).as_deref(), turns[1]["reply"].as_str());
    assert_eq!(f.request(lead).provider_turn_id.as_deref(), Some("1"));
    assert_eq!(f.request(next).provider_turn_id.as_deref(), Some("2"));
    assert_eq!(f.worker.state.agent(f.agent).unwrap().current_request, None);
    // Unrelated manual Started callbacks also start a monotonic own-turn
    // grace. Expire that fixture-owned guard using the recorded gap before
    // 36143, keeping the replay independent of the host monotonic epoch.
    let last_manual_start = turns.last().unwrap()["rows"][0]["time"].as_u64().unwrap();
    let elapsed = incident["later"]["submitted_at_ms"].as_u64().unwrap() - last_manual_start;
    assert!(Duration::from_millis(elapsed) >= super::super::super::OWN_TURN_GRACE);
    // That recorded time jump expires this test-owned guard. Do not subtract
    // 85 minutes from a host Instant: its representable epoch is platform-bound.
    assert!(f.worker.own_turns.remove(&f.agent).is_some());
    let later = send_recorded(&mut f, &incident["later"], &clock);
    assert_eq!(later, RequestId(36143));
    f.deliver();
    assert_eq!(f.request(later).phase, RequestPhase::Active);
    assert_eq!(f.request(later).failure_reason, None);
    assert_eq!(
        f.typed().len(),
        3,
        "later delivery drains without recovery or replay"
    );
}

#[test]
fn kimi_incident_34948_and_36279_require_the_recorded_guidance_attestation() {
    let incidents = guidance_cases();
    for (case_number, incident) in incidents.iter().enumerate() {
        for control in [
            "recorded",
            "another_active_prompt",
            "duplicate_ids",
            "repeated_message_id",
            "foreign_text",
        ] {
            let mut f = Fixture::new(Provider::Kimi);
            let wire = home(&f);
            let clock = ReplayClock::new(&f, &incident["lead"]);
            let lead = start_recorded(&mut f, &incident["lead"], "0", &clock);
            let follow = send_recorded(&mut f, &incident["follow"], &clock);
            f.deliver();
            assert_eq!(
                f.typed()[1].0,
                incident["follow"]["payload"].as_str().unwrap()
            );
            let mut rows = incident["rows"].as_array().unwrap().clone();
            let guidance_index = rows
                .iter()
                .position(|row| row["type"] == "prompt.steered")
                .unwrap();
            match control {
                "another_active_prompt" => {
                    rows[guidance_index]["activePromptId"] =
                        incidents[1 - case_number]["rows"][0]["promptId"].clone();
                }
                "duplicate_ids" => {
                    let guidance = &mut rows[guidance_index];
                    let id = guidance["promptIds"][0].clone();
                    let text = guidance["content"][0].clone();
                    guidance["promptIds"] = json!([id, id]);
                    guidance["content"] = json!([text, text]);
                }
                "repeated_message_id" => {
                    let mut prior = rows[guidance_index].clone();
                    prior["content"][0]["text"] =
                        incidents[1 - case_number]["follow"]["payload"].clone();
                    rows.insert(guidance_index, prior);
                }
                "foreign_text" => {
                    rows[guidance_index]["content"][0]["text"] =
                        incidents[1 - case_number]["follow"]["payload"].clone();
                }
                _ => {}
            }
            std::fs::write(&wire, wire_rows(&rows)).unwrap();
            f.now = clock.at(rows.last().unwrap()["time"].as_u64().unwrap());
            f.status(RuntimeStatus::Idle);
            stop(&f);
            f.consume();
            assert_eq!(f.reply(lead).as_deref(), incident["reply"].as_str());
            assert_eq!(
                f.reply(follow).as_deref(),
                (control == "recorded").then(|| incident["reply"].as_str().unwrap()),
                "request {follow:?}, control {control}"
            );
            if control == "recorded" {
                assert_eq!(f.request(follow).provider_turn_id.as_deref(), Some("0"));
                assert!(f.request(follow).trusted_start_bound);
            } else {
                assert!(!f.request(follow).trusted_start_bound);
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
            }
            f.deliver();
            assert_eq!(f.typed().len(), 2, "no repeat of request {follow:?}");
        }
    }
}

struct RecordedEditorUncertainty(Arc<Mutex<Vec<Method>>>, String);

impl Transport for RecordedEditorUncertainty {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        let input = matches!(
            &method,
            Method::AgentPromptIfIdle(_) | Method::AgentPromptIfIdleAtGeneration(_)
        );
        let response = Native(self.0.clone()).request(method)?;
        if input {
            Err(TransportError {
                code: None,
                message: self.1.clone(),
                definitely_rejected: false,
            })
        } else {
            Ok(response)
        }
    }
}

#[test]
fn kimi_incident_32629_unclaimed_uncertainty_keeps_native_detail_without_replay() {
    let incident: Value = serde_json::from_str(include_str!(
        "fixtures/kimi_reliability/sequence-unclaimed.json"
    ))
    .unwrap();
    let mut f = Fixture::new(Provider::Kimi);
    home(&f);
    let clock = ReplayClock::new(&f, &incident["lead"]);
    let lead = start_recorded(&mut f, &incident["lead"], "0", &clock);
    let native_error = incident["native_error"].as_str().unwrap();
    f.worker.transport = Box::new(RecordedEditorUncertainty(
        f.native.clone(),
        native_error.into(),
    ));
    let follow = send_recorded(&mut f, &incident["follow"], &clock);
    f.deliver();
    assert_eq!(follow, RequestId(32629));
    assert!(f.request(follow).uncertain_outcome);
    assert_eq!(
        f.request(follow).submission_error.as_deref(),
        Some(native_error)
    );
    // This bundle retains the original committed final in Bus history, but
    // not its native wire. Replay that final at the model callback boundary;
    // do not invent a native prompt ID or a guidance witness for the follower.
    let mut state = f.worker.state.clone();
    let mut final_event = ProviderCallback::final_event(
        "recorded-32525-final",
        100,
        f.agent,
        "launch",
        "session",
        "0",
        incident["lead"]["payload"].as_str().unwrap(),
        incident["reply"].as_str().unwrap(),
    );
    final_event.occurred_at_ms = clock.at(incident["completed_at_ms"].as_u64().unwrap());
    let disposition = state.accept_callback(final_event);
    assert!(
        !matches!(
            disposition,
            crate::messaging::model::CallbackDisposition::Rejected(_)
        ),
        "{disposition:?}"
    );
    f.worker.save(state).unwrap();
    f.now = clock.at(incident["completed_at_ms"].as_u64().unwrap());
    f.status(RuntimeStatus::Idle);
    let mut state = f.worker.state.clone();
    assert!(state.settle_unclaimed_steering(f.agent, f.now, |_| None));
    f.worker.save(state).unwrap();
    assert_eq!(f.reply(lead).as_deref(), incident["reply"].as_str());
    assert!(f.reply(follow).is_none());
    assert!(
        !f.request(follow).settled(),
        "retain unclaimed input for its existing evidence window"
    );
    f.now += crate::messaging::model::DELIVERED_STALL_MS + 1;
    f.status(RuntimeStatus::Idle);
    let mut state = f.worker.state.clone();
    assert!(state.settle_unclaimed_steering(f.agent, f.now, |_| None));
    f.worker.save(state).unwrap();
    let status = f.worker.control_message_at(PromptId(32628), f.now).unwrap();
    assert_eq!(status["requests"][0]["reason"], "submission_unconfirmed");
    assert_eq!(status["requests"][0]["uncertain_outcome"], true);
    assert!(status["requests"][0]["reply"].is_null());
    assert!(status["requests"][0]["detail"]
        .as_str()
        .unwrap()
        .contains(native_error));
    assert_eq!(
        f.request(follow).submission_error.as_deref(),
        Some(native_error)
    );
    f.deliver();
    assert_eq!(
        f.typed().len(),
        2,
        "uncertain input is never automatically pasted again"
    );
}
