#[test]
fn yolo_attach_requires_the_same_run_mode_as_the_server() {
    let _guard = env_lock().lock().unwrap();
    let _bus = crate::utils::config::test_without_bus_env(&_guard);
    let dir = unique_test_dir("yolo-attach");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("api.sock");
    std::env::set_var(crate::protocol::api::SOCKET_PATH_ENV_VAR, &path);
    for (requested, serving, want_ok) in [
        (false, true, false),
        (true, false, false),
        (false, false, true),
        (true, true, true),
    ] {
        if requested {
            std::env::set_var("BUS_YOLO", "1");
        } else {
            std::env::remove_var("BUS_YOLO");
        }
        let listener = UnixListener::bind(&path).unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            let body = serde_json::json!({"id":"autodetect:server:status","result":{"type":"pong","version":"test","protocol":crate::protocol::wire::PROTOCOL_VERSION,"capabilities":{"endpoint_protocol_generation":crate::protocol::wire::handshake::ENDPOINT_PROTOCOL_GENERATION,"yolo":serving}}});
            writeln!(stream, "{body}").unwrap();
        });
        let result = validate_running_server_compatibility(false);
        handle.join().unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            result.is_ok(),
            want_ok,
            "requested={requested}, serving={serving}: {result:?}"
        );
    }
    std::env::remove_var("BUS_YOLO");
    std::env::remove_var(crate::protocol::api::SOCKET_PATH_ENV_VAR);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn yolo_resumed_codex_master_uses_current_run_without_persisting() {
    use crate::agents::{
        providers::{hook_json, spool, ProviderKind},
        resume::catalog::{self, AgentSessionRef, PersistedAgentSession},
    };
    use crate::messaging::{
        coordinator::resume::{self, NativeResumeContext, NativeResumeFacts},
        identity,
        model::{AgentRuntimeIdentity, BusState, Provider},
        storage::state_store::JsonStore,
    };

    struct Cleanup {
        root: PathBuf,
        previous_yolo: Option<std::ffi::OsString>,
    }
    impl Drop for Cleanup {
        fn drop(&mut self) {
            match &self.previous_yolo {
                Some(value) => std::env::set_var("BUS_YOLO", value),
                None => std::env::remove_var("BUS_YOLO"),
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    let _guard = env_lock().lock().unwrap();
    let cleanup = Cleanup {
        root: crate::utils::test_temp::unique_temp_path("codex-master-resume"),
        previous_yolo: std::env::var_os("BUS_YOLO"),
    };
    let project = cleanup.root.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let binary = std::env::current_exe().unwrap();
    let mut state = BusState::new();
    let room = state.create_room("saved-work").unwrap();
    let master = state.ensure_master_room();
    // A saved MASTER fixture bypasses only new-agent admission, which remains refused.
    assert!(crate::messaging::orchestration::check_new_orchestrator(Provider::Codex).is_err());
    let agent = state
        .create_agent(
            master,
            "saved-master",
            Provider::Codex,
            project.clone(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(agent, room).unwrap();
    state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("saved-master-launch".into()),
                terminal_id: Some("saved-terminal".into()),
                pane_id: Some("saved-pane".into()),
                session_id: Some("saved-conversation".into()),
            },
        )
        .unwrap();
    JsonStore::new(cleanup.root.join("state.json"))
        .save(&state)
        .unwrap();
    let capture = cleanup.root.join("callbacks/saved-master-launch");
    spool::initialize(
        &capture,
        &spool::Manifest {
            routing_key: spool::RoutingKey(agent.0),
            provider: ProviderKind::Codex,
            launch_id: "saved-master-launch".into(),
        },
    )
    .unwrap();
    hook_json::install_hooks(
        &project.join(".codex/hooks.json"),
        hook_json::HookContract::for_provider(ProviderKind::Codex),
        &binary,
    )
    .unwrap();
    let prompt =
        crate::messaging::orchestration::write_prompt(&capture, "Coordinate saved-work.").unwrap();
    let session = PersistedAgentSession {
        source: "herdr:codex".into(),
        agent: "codex".into(),
        session_ref: AgentSessionRef::id("saved-conversation").unwrap(),
    };
    let plan = catalog::plan(&session.source, &session.agent, &session.session_ref).unwrap();
    let facts = NativeResumeFacts {
        agent_name: Some(identity::managed_name(master, agent)),
        managed_agent: Some("codex".into()),
        session: Some(session),
    };
    let context = NativeResumeContext {
        root: Some(cleanup.root.clone()),
        session_name: Some("bus".into()),
        config_root: cleanup.root.join("herdr-config"),
        project,
        binary,
    };
    for explicit in [
        None,
        Some("--yolo"),
        Some("--dangerously-bypass-approvals-and-sandbox"),
    ] {
        let options: Vec<&str> = explicit.into_iter().collect();
        let saved = serde_json::to_vec(&options).unwrap();
        std::fs::write(capture.join("launch-options.json"), &saved).unwrap();
        for enabled in [true, false, true] {
            if enabled {
                std::env::set_var("BUS_YOLO", "1");
            } else {
                std::env::remove_var("BUS_YOLO");
            }
            let extras = resume::load(&context, &facts, &plan).unwrap();
            let bypass: Vec<_> = extras
                .args
                .iter()
                .filter(|arg| {
                    matches!(
                        arg.as_str(),
                        "--yolo" | "--dangerously-bypass-approvals-and-sandbox"
                    )
                })
                .map(String::as_str)
                .collect();
            let expected: Vec<_> = explicit
                .or(enabled.then_some("--yolo"))
                .into_iter()
                .collect();
            assert_eq!(bypass, expected, "enabled={enabled}, explicit={explicit:?}");
            assert!(extras.args.iter().any(|arg| arg == "--no-daemon"));
            assert!(extras.args.iter().any(|arg| {
                arg.starts_with("developer_instructions=")
                    && arg.contains(&prompt.display().to_string())
            }));
            assert_eq!(
                std::fs::read(capture.join("launch-options.json")).unwrap(),
                saved
            );
        }
    }
}
