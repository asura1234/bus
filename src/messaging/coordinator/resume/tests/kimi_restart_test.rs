#[cfg(target_os = "macos")]
use crate::agents::providers::kimi::launch::update_test::resume_home_test::previous_mount;
use crate::agents::providers::{
    kimi::launch::update_test::{resume_home_test::use_home, Fixture},
    ProviderKind,
};

#[test]
fn restart_still_binds_the_bus_owned_exact_session() {
    use crate::agents::{
        providers::spool::{self, Manifest, RoutingKey},
        resume::catalog::{self, AgentSessionRef, PersistedAgentSession},
    };
    use crate::messaging::{
        coordinator::resume::{load, NativeResumeContext, NativeResumeFacts},
        identity,
        model::{AgentRuntimeIdentity, BusState, Provider},
        storage::state_store::JsonStore,
    };
    let fixture = Fixture::new();
    fixture.prepare();
    let home = use_home(&fixture);
    #[cfg(target_os = "macos")]
    previous_mount(&fixture, true);
    let mut state = BusState::new();
    let room = state.create_room("restart").unwrap();
    let agent = state
        .create_agent(room, "owner", Provider::Kimi, fixture.root.clone(), None)
        .unwrap();
    state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                session_id: Some("owned-session".into()),
                ..Default::default()
            },
        )
        .unwrap();
    JsonStore::new(fixture.root.join("state.json"))
        .save(&state)
        .unwrap();
    spool::initialize(
        &fixture.hooks.spool,
        &Manifest {
            routing_key: RoutingKey(agent.0),
            provider: ProviderKind::Kimi,
            launch_id: "launch".into(),
        },
    )
    .unwrap();
    let context = NativeResumeContext {
        root: Some(fixture.root.clone()),
        session_name: Some("bus".into()),
        config_root: fixture.root.join("herdr-config"),
        project: fixture.root.clone(),
        binary: fixture.hooks.binary.clone(),
    };
    for native_session in ["owned-session", "stale-native-session"] {
        let session = PersistedAgentSession {
            source: "herdr:kimi".into(),
            agent: "kimi".into(),
            session_ref: AgentSessionRef::id(native_session).unwrap(),
        };
        let plan = catalog::plan(&session.source, &session.agent, &session.session_ref).unwrap();
        let mut facts = NativeResumeFacts {
            agent_name: Some(identity::managed_name(room, agent)),
            managed_agent: Some("kimi".into()),
            session: Some(session.clone()),
        };
        let extras = load(&context, &facts, &plan).unwrap();
        assert_eq!(
            extras.session.unwrap_or(session).session_ref.value,
            "owned-session"
        );
        assert!(extras
            .env
            .contains(&("KIMI_CODE_HOME".into(), home.to_string_lossy().into_owned())));
        facts.agent_name = Some("bus-r1-a999".into());
        assert!(load(&context, &facts, &plan).is_err());
    }
}

#[test]
fn kimi_restore_preserves_a_persisted_provider_hold_until_its_original_request_can_retry() {
    use crate::agents::{
        providers::spool::{self, Manifest, RoutingKey},
        resume::catalog::{self, AgentSessionRef, PersistedAgentSession},
    };
    use crate::messaging::{
        coordinator::{
            resume::{load, NativeResumeContext, NativeResumeFacts},
            Worker,
        },
        identity,
        model::{
            AgentRuntimeIdentity, BusState, CallbackDisposition, CallbackEventKind, Provider,
            ProviderCallback, RuntimeStatus, SubmissionOutcome,
        },
        native::{Transport, TransportError},
        storage::state_store::JsonStore,
    };
    use crate::protocol::api::schema::{AgentInfo, Method, ResponseResult};
    let fixture = Fixture::new();
    crate::platform::fs::private_dir(&fixture.root).unwrap();
    fixture.prepare();
    let home = use_home(&fixture);
    #[cfg(target_os = "macos")]
    previous_mount(&fixture, true);
    let mut state = BusState::new();
    let room = state.create_room("held-restart").unwrap();
    let agent = state
        .create_agent(room, "owner", Provider::Kimi, fixture.root.clone(), None)
        .unwrap();
    state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                session_id: Some("owned-session".into()),
                ..Default::default()
            },
        )
        .unwrap();
    state.confirm_hook_setup(agent).unwrap();
    state
        .observe_status(agent, RuntimeStatus::Idle, 10)
        .unwrap();
    state
        .set_draft_text(room, "finish the retained task")
        .unwrap();
    state.set_draft_recipients(room, [agent]).unwrap();
    let request = state.submit_draft(room, 10).unwrap()[0];
    state.begin_submission(request, "launch", 10).unwrap();
    state
        .record_submission(
            request,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("owned-session".into()),
                provider_turn_id: Some("7".into()),
            },
        )
        .unwrap();
    let started = ProviderCallback {
        callback_id: "held-start".into(),
        sequence: 11,
        occurred_at_ms: 11,
        agent_id: agent,
        launch_id: "launch".into(),
        provider_session_id: Some("owned-session".into()),
        provider_turn_id: Some("7".into()),
        provider_prompt_id: None,
        prompt_payload: Some(state.request(request).unwrap().prompt.rendered_payload()),
        kind: CallbackEventKind::PromptStarted,
    };
    assert_eq!(
        state.accept_callback(started.clone()),
        CallbackDisposition::AcceptedBinding
    );
    assert_eq!(
        state.accept_callback(ProviderCallback {
            callback_id: "held-error".into(),
            sequence: 12,
            occurred_at_ms: 100,
            prompt_payload: None,
            kind: CallbackEventKind::Error {
                message: "Provider failed".into(),
                provider_error: Some("HTTP 503 Service Unavailable".into()),
            },
            ..started
        }),
        CallbackDisposition::AcceptedError
    );
    let hold = state.agent(agent).unwrap().provider_retry.clone().unwrap();
    let store = JsonStore::new(fixture.root.join("state.json"));
    store.save(&state).unwrap();
    spool::initialize(
        &fixture.hooks.spool,
        &Manifest {
            routing_key: RoutingKey(agent.0),
            provider: ProviderKind::Kimi,
            launch_id: "launch".into(),
        },
    )
    .unwrap();
    let session = PersistedAgentSession {
        source: "herdr:kimi".into(),
        agent: "kimi".into(),
        session_ref: AgentSessionRef::id("stale-native-session").unwrap(),
    };
    let plan = catalog::plan(&session.source, &session.agent, &session.session_ref).unwrap();
    let facts = NativeResumeFacts {
        agent_name: Some(identity::managed_name(room, agent)),
        managed_agent: Some("kimi".into()),
        session: Some(session),
    };
    let context = NativeResumeContext {
        root: Some(fixture.root.clone()),
        session_name: Some("bus".into()),
        config_root: fixture.root.join("herdr-config"),
        project: fixture.root.clone(),
        binary: fixture.hooks.binary.clone(),
    };
    let before = std::fs::read(store.path()).unwrap();
    let extras = load(&context, &facts, &plan).unwrap();
    assert_eq!(extras.session.unwrap().session_ref.value, "owned-session");
    assert!(extras
        .env
        .contains(&("KIMI_CODE_HOME".into(), home.to_string_lossy().into_owned())));
    assert_eq!(
        std::fs::read(store.path()).unwrap(),
        before,
        "native restore must not rewrite the hold"
    );
    struct RestoredKimi(AgentInfo);
    impl Transport for RestoredKimi {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            match method {
                Method::AgentListWithDetectionContext(_) => Ok(ResponseResult::AgentList {
                    agents: vec![self.0.clone()],
                }),
                other => panic!("unexpected restore operation: {other:?}"),
            }
        }
    }
    let native = serde_json::from_value(serde_json::json!({
        "terminal_id":"restored-terminal", "pane_id":"restored-pane",
        "name":identity::managed_name(room, agent), "agent":"kimi", "agent_status":"idle",
        "workspace_id":"w1", "tab_id":"t1", "focused":false, "revision":1,
        "interactive_ready":true, "native_ready":true,
        "agent_session":{"source":"herdr:kimi", "agent":"kimi", "kind":"id", "value":"owned-session"}
    })).unwrap();
    let mut worker = Worker::open(fixture.root.clone(), Box::new(RestoredKimi(native))).unwrap();
    assert_eq!(
        worker.state.agent(agent).unwrap().provider_retry.as_ref(),
        Some(&hold)
    );
    worker.poll().unwrap().unwrap();
    let owner = worker.state.agent(agent).unwrap();
    assert_eq!(
        owner.runtime_identity.pane_id.as_deref(),
        Some("restored-pane")
    );
    assert_eq!(
        owner.runtime_identity.session_id.as_deref(),
        Some("owned-session")
    );
    assert_eq!(owner.provider_retry.as_ref(), Some(&hold));
    assert_eq!(owner.current_request, Some(request));
    assert_eq!(owner.shown_status(), RuntimeStatus::Working);
    assert!(!worker.state.request(request).unwrap().settled());
    let mut persisted = store.load().unwrap().unwrap();
    assert_eq!(
        persisted.agent(agent).unwrap().provider_retry.as_ref(),
        Some(&hold)
    );
    assert!(persisted.retry_provider_requests(hold.retry_at_ms));
    assert_eq!(persisted.queued_requests(agent), &[request]);
    assert_eq!(
        persisted.agent(agent).unwrap().shown_status(),
        RuntimeStatus::Working
    );
}
