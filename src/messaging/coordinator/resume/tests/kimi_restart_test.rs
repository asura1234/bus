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
