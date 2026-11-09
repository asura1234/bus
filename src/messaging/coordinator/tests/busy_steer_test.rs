use super::*;

/// A turn Bus already submitted, still Working, before its start hook binds.
fn busy_untrusted_turn(fixture: &mut Fixture) -> RequestId {
    let lead = fixture.send("write the parser", false);
    fixture.deliver();
    fixture.status(RuntimeStatus::Working);
    assert!(
        !fixture.request(lead).trusted_start_bound,
        "the start hook has not bound"
    );
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
    lead
}

#[test]
fn a_busy_agent_receives_the_message_without_waiting_for_idle() {
    for provider in [Provider::Codex, Provider::Cursor, Provider::Kimi] {
        let mut fixture = Fixture::new(provider);
        let lead = busy_untrusted_turn(&mut fixture);
        let follow = fixture.send("also add tests", false);
        fixture.worker.note_requests_queued(&[follow]);
        fixture.deliver();
        assert_eq!(
            fixture.worker.state.agent(fixture.agent).unwrap().status,
            RuntimeStatus::Working,
            "{provider:?} must be steered while it is still busy"
        );
        assert!(
            !fixture
                .worker
                .state
                .agent(fixture.agent)
                .unwrap()
                .interrupt_pending,
            "{provider:?} a routine message must not interrupt the turn"
        );
        assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
        assert_eq!(fixture.request(follow).group, Some(lead));
        assert!(fixture.request(follow).steered);
        assert_eq!(fixture.request(follow).phase, RequestPhase::Active);
        assert_eq!(
            fixture
                .typed_content()
                .last()
                .map(|(text, steer)| (text.as_str(), *steer)),
            Some(("also add tests", true)),
            "{provider:?}"
        );
        assert!(fixture.request(follow).latency.picked_at_ms.is_some());
        assert_eq!(
            crate::messaging::dashboard::ready_queue_sample(fixture.request(follow)),
            None,
            "{provider:?} a steered delivery has no ready_queue phase"
        );
    }
}

#[test]
fn a_busy_agent_with_no_bus_turn_is_steered_without_waiting_for_idle() {
    for provider in [Provider::Codex, Provider::Cursor, Provider::Kimi] {
        let mut fixture = Fixture::new(provider);
        let follow = fixture.send("while you work", false);
        assert!(
            fixture.request(follow).latency.ready_at_enqueue,
            "enqueued before the agent became busy"
        );
        fixture.worker.note_requests_queued(&[follow]);
        fixture.status(RuntimeStatus::Working);
        fixture.deliver();
        assert_eq!(
            fixture.typed_content(),
            [("while you work".to_owned(), true)],
            "{provider:?}"
        );
        assert_eq!(
            fixture.worker.state.agent(fixture.agent).unwrap().status,
            RuntimeStatus::Working
        );
        assert!(fixture.request(follow).latency.agent_waited);
        assert!(fixture.request(follow).latency.picked_at_ms.is_some());
        assert_eq!(
            crate::messaging::dashboard::ready_queue_sample(fixture.request(follow)),
            None,
            "{provider:?}"
        );
    }
}

#[test]
fn two_messages_to_a_busy_agent_steer_in_order_and_only_once() {
    let mut fixture = Fixture::new(Provider::Codex);
    let lead = busy_untrusted_turn(&mut fixture);
    let first = fixture.send("first correction", false);
    let second = fixture.send("second correction", false);
    fixture.deliver();
    assert_eq!(
        fixture
            .typed_content()
            .last()
            .map(|(text, steer)| (text.as_str(), *steer)),
        Some(("first correction", true)),
        "the later message must not overtake the queue head"
    );
    assert_eq!(
        fixture.worker.state.queued_requests(fixture.agent),
        &[second]
    );
    assert_eq!(fixture.request(first).group, Some(lead));
    fixture.deliver();
    assert_eq!(
        fixture
            .typed_content()
            .iter()
            .rev()
            .take(2)
            .map(|(text, steer)| (text.as_str(), *steer))
            .collect::<Vec<_>>(),
        vec![("second correction", true), ("first correction", true)]
    );
    assert!(fixture
        .worker
        .state
        .queued_requests(fixture.agent)
        .is_empty());
    let typed = fixture.typed().len();
    fixture.deliver();
    assert_eq!(
        fixture.typed().len(),
        typed,
        "a steered message is not delivered again"
    );
    assert_eq!(fixture.request(first).phase, RequestPhase::Active);
    assert_eq!(fixture.request(second).phase, RequestPhase::Active);
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
}

#[test]
fn no_prompt_or_a_closed_submission_window_keeps_the_message_queued() {
    let mut launching = Fixture::new(Provider::Codex);
    launching.status(RuntimeStatus::Launching);
    let waiting = launching.send("before the prompt exists", false);
    launching.deliver();
    assert!(launching.typed().is_empty(), "no prompt yet");
    assert_eq!(launching.request(waiting).phase, RequestPhase::Queued);
    assert!(!launching.request(waiting).steered);

    let mut window = Fixture::new(Provider::Codex);
    window.worker.transport = Box::new(ClosedWindow(window.native.clone()));
    let lead = window.send("write the parser", false);
    window.deliver();
    window.status(RuntimeStatus::Working);
    assert_eq!(
        window.request(lead).phase,
        RequestPhase::Submitting,
        "the native write is still unconfirmed"
    );
    let follow = window.send("also add tests", false);
    window.deliver();
    assert_eq!(window.typed().len(), 1, "do not type over a closed window");
    assert_eq!(window.request(follow).phase, RequestPhase::Queued);
    assert!(!window.request(follow).steered);

    let mut grok = Fixture::new(Provider::Grok);
    grok.status(RuntimeStatus::Working);
    let held = grok.send("after this turn", false);
    grok.deliver();
    assert!(
        grok.typed().is_empty(),
        "Grok refuses a follow-up typed during the turn"
    );
    assert_eq!(grok.request(held).phase, RequestPhase::Queued);
}

struct ClosedWindow(Arc<Mutex<Vec<Method>>>);

impl Transport for ClosedWindow {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        self.0.lock().unwrap().push(method);
        Ok(ResponseResult::Ok {})
    }
}
