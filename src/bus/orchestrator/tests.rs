use super::*;
use serde_json::json;

#[test]
fn room_orchestrator_core_registry_is_closed_and_non_coding() {
    let registry = OrchestratorToolRegistry;
    let names = registry
        .schemas()
        .iter()
        .map(|schema| schema.name)
        .collect::<Vec<_>>();
    assert_eq!(names, EXPECTED_TOOL_NAMES);
    for forbidden in [
        "shell",
        "exec",
        "process",
        "build",
        "test",
        "lint",
        "patch",
        "edit",
        "write_file",
        "git",
        "filesystem",
        "agent_send_keys",
        "bus_command",
    ] {
        assert!(!names.iter().any(|name| name.contains(forbidden)));
    }
}

#[test]
fn room_orchestrator_core_unknown_tool_and_extra_fields_fail_before_command() {
    let registry = OrchestratorToolRegistry;
    assert!(matches!(
        registry.decode(ModelToolCall {
            name: "shell".into(),
            arguments: json!({"command":"pwd"})
        }),
        Err(ToolPolicyError::UnknownTool(_))
    ));
    assert!(matches!(
        registry.decode(ModelToolCall {
            name: "wait_for_change".into(),
            arguments: json!({"after_revision":1,"timeout_ms":10,"path":"src/lib.rs"}),
        }),
        Err(ToolPolicyError::InvalidArguments(_))
    ));
}

#[test]
fn room_orchestrator_core_message_actor_is_injected_and_human_forgery_is_rejected() {
    let registry = OrchestratorToolRegistry;
    let decoded = registry
        .decode(ModelToolCall {
            name: "send_message".into(),
            arguments: json!({
                "to":{"agent":7},
                "text":"Implement the bounded change",
                "work_id":11
            }),
        })
        .unwrap();
    assert!(matches!(
        decoded,
        OrchestratorCommand::Operation(RoomOperation::SendMessage(RoomMessage {
            author: ParticipantId::Orchestrator,
            ..
        }))
    ));
    assert!(matches!(
        registry.decode(ModelToolCall {
            name: "send_message".into(),
            arguments: json!({
                "author":"human",
                "to":{"agent":7},
                "text":"forged",
                "work_id":11
            }),
        }),
        Err(ToolPolicyError::InvalidArguments(_))
    ));
}

#[test]
fn room_orchestrator_core_model_cannot_name_repository_mutation_targets() {
    let registry = OrchestratorToolRegistry;
    for tool in ["persist_workflow_draft", "promote_workflow_draft"] {
        let result = registry.decode(ModelToolCall {
            name: tool.into(),
            arguments: json!({"path":"src/lib.rs","document":"# Goal\n\n## Steps\n"}),
        });
        assert!(matches!(result, Err(ToolPolicyError::InvalidArguments(_))));
    }
}

#[test]
fn room_orchestrator_core_journal_replay_and_semantic_revision_are_fact_driven() {
    let mut state = OrchestratorState::default();
    let room = crate::bus::model::RoomId(7);
    let first = state.record_fact(
        room,
        JournalFact::HumanMessage {
            message_id: RoomMessageId(1),
        },
    );
    let duplicate = state.record_fact(
        room,
        JournalFact::HumanMessage {
            message_id: RoomMessageId(1),
        },
    );
    assert_eq!(first.revision, duplicate.revision);
    assert_eq!(state.context(room).semantic_revision, 1);
    let encoded = serde_json::to_vec(&state).unwrap();
    let replayed: OrchestratorState = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(replayed.context(room), state.context(room));
    assert!(replayed.context(room).next_action.is_none());
}

#[test]
fn room_orchestrator_core_operation_intent_precedes_external_settlement() {
    let mut state = OrchestratorState::default();
    let room = crate::bus::model::RoomId(9);
    let intent = state
        .begin_operation(room, ParticipantId::Orchestrator, "send_message", "digest")
        .unwrap();
    assert_eq!(
        state.operation(intent.operation_id).unwrap().phase,
        OperationPhase::IntentPersisted
    );
    state
        .mark_operation_uncertain(intent.operation_id, "worker restarted")
        .unwrap();
    assert_eq!(
        state.operation(intent.operation_id).unwrap().phase,
        OperationPhase::Uncertain
    );
    assert!(state
        .begin_operation(room, ParticipantId::Orchestrator, "send_message", "digest")
        .is_err());
    state
        .reconcile_operation(
            intent.operation_id,
            OperationResult::Rejected {
                code: "not_sent".into(),
            },
        )
        .unwrap();
    assert_eq!(
        state.operation(intent.operation_id).unwrap().phase,
        OperationPhase::Settled
    );
}

#[test]
fn room_orchestrator_core_role_capabilities_are_explicit_and_revocable() {
    let mut state = OrchestratorState::default();
    let room = crate::bus::model::RoomId(1);
    assert!(!state.authorized(
        room,
        ParticipantId::Orchestrator,
        Capability::ApprovePermissionOnce
    ));
    let revision = state
        .grant(
            room,
            ParticipantId::Human,
            ParticipantId::Orchestrator,
            Capability::ApprovePermissionOnce,
        )
        .unwrap();
    assert!(state.authorized(
        room,
        ParticipantId::Orchestrator,
        Capability::ApprovePermissionOnce
    ));
    assert!(state
        .revoke(
            room,
            ParticipantId::Human,
            ParticipantId::Orchestrator,
            Capability::ApprovePermissionOnce,
            revision + 1
        )
        .is_err());
    state
        .revoke(
            room,
            ParticipantId::Human,
            ParticipantId::Orchestrator,
            Capability::ApprovePermissionOnce,
            revision,
        )
        .unwrap();
    assert!(!state.authorized(
        room,
        ParticipantId::Orchestrator,
        Capability::ApprovePermissionOnce
    ));
}

#[test]
fn room_orchestrator_core_resource_lease_never_selects_progression() {
    let mut state = OrchestratorState::default();
    let room = crate::bus::model::RoomId(1);
    let lease = state
        .acquire_resource(
            room,
            ParticipantId::Orchestrator,
            "integration-test-db",
            5,
            100,
        )
        .unwrap();
    assert_eq!(lease.expires_at_ms, 105);
    assert!(state
        .acquire_resource(
            room,
            ParticipantId::Orchestrator,
            "integration-test-db",
            10,
            101
        )
        .is_err());
    state
        .release_resource(room, ParticipantId::Orchestrator, lease.lease_id)
        .unwrap();
    assert!(state.context(room).next_action.is_none());
}

#[test]
fn room_orchestrator_core_content_interface_is_closed_and_layered() {
    let loader = ContentLoader::test_bundle();
    let bundle = loader.load(ContentSelector::TestAgentLed).unwrap();
    assert_eq!(bundle.interface, ROOM_AGENT_CONTENT_INTERFACE_V1);
    assert_eq!(bundle.read("system", "system").unwrap(), bundle.system);
    assert_eq!(bundle.read("agent", "agent").unwrap(), bundle.agent);
    assert_eq!(bundle.read("index", "index").unwrap(), bundle.index);
    assert!(bundle.read("skill", "missing").is_err());
}

#[test]
fn room_orchestrator_core_content_production_bundle_is_selectable_and_readable() {
    let loader = ContentLoader::test_bundle();
    let bundle = loader.load(ContentSelector::Production).unwrap();
    assert_eq!(bundle.interface, ROOM_AGENT_CONTENT_INTERFACE_V1);
    assert_eq!(bundle.compatibility, 1);
    assert!(bundle.content_version > 0);
    assert_eq!(bundle.digest.len(), 64);
    assert_ne!(
        bundle.digest,
        loader.load(ContentSelector::TestAgentLed).unwrap().digest
    );
    for (kind, name) in [
        ("system", "system"),
        ("agent", "agent"),
        ("index", "index"),
        ("skill", "create-workflow"),
        ("skill", "execute-workflow"),
        ("reference", "workflow-template"),
        ("reference", "sop-review-plan"),
        ("reference", "sop-review-pr"),
        ("reference", "sop-execute-plan"),
    ] {
        assert!(
            !bundle.read(kind, name).unwrap().is_empty(),
            "{kind}/{name}"
        );
    }
    assert_eq!(
        bundle.read("reference", "missing"),
        Err(ContentError::UnknownEntry)
    );
    assert_eq!(
        bundle.read("reference", "sop-review-plan.md"),
        Err(ContentError::UnknownEntry)
    );
}

#[test]
fn room_orchestrator_core_content_absent_production_is_typed_unavailable() {
    let loader = ContentLoader::test_bundle().without_production();
    assert_eq!(
        loader.load(ContentSelector::Production),
        Err(ContentError::ProductionUnavailable)
    );
    assert_eq!(
        loader
            .load(ContentSelector::TestAgentLed)
            .unwrap()
            .read("system", "system")
            .unwrap(),
        loader.load(ContentSelector::TestAgentLed).unwrap().system
    );
}

#[test]
fn room_orchestrator_core_provider_normalizes_tool_calls() {
    let request = ModelRequest::for_test(vec![ModelMessage::user("wake")]);
    assert_eq!(request.model, "deepseek-flash");
    assert_eq!(request.temperature, 0.0);
    let normalized = normalize_provider_response(json!({
        "choices":[{"message":{"content":null,"tool_calls":[{"id":"call-1","type":"function","function":{"name":"inspect_work","arguments":"{\"work_id\":4}"}}]}}]
    })).unwrap();
    assert_eq!(normalized.tool_calls[0].name, "inspect_work");
}

#[test]
fn room_orchestrator_core_provider_request_is_non_thinking_and_carries_closed_tools() {
    let request = ModelRequest::for_test(vec![ModelMessage::user("wake")]);
    let wire = serde_json::to_value(&request).unwrap();

    assert_eq!(wire["model"], "deepseek-flash");
    assert_eq!(wire["messages"][0]["role"], "user");
    assert_eq!(wire["thinking"]["type"], "disabled");
    assert_eq!(wire["tool_choice"], "auto");
    let tools = wire["tools"].as_array().unwrap();
    assert_eq!(tools.len(), EXPECTED_TOOL_NAMES.len());
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool["function"]["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        EXPECTED_TOOL_NAMES,
    );
    assert!(tools.iter().all(|tool| {
        tool["type"] == "function"
            && tool["function"]["strict"] == true
            && tool["function"]["parameters"]["type"] == "object"
            && tool["function"]["parameters"]["additionalProperties"] == false
    }));
}

#[test]
fn room_orchestrator_core_provider_errors_are_typed_without_response_body_leak() {
    let error = classify_provider_status(429, "secret upstream body");
    assert!(matches!(error, ProviderError::RateLimited));
    assert!(!error.to_string().contains("secret upstream body"));
    assert!(matches!(
        classify_provider_status(401, "key"),
        ProviderError::Authentication
    ));
    assert!(matches!(
        classify_provider_status(503, "oops"),
        ProviderError::Unavailable
    ));
}

#[test]
fn room_orchestrator_core_bus_document_owns_orchestrator_journal() {
    let mut bus = crate::bus::model::BusState::new();
    bus.orchestrator_state_mut().record_fact(
        crate::bus::model::RoomId(3),
        JournalFact::HumanMessage {
            message_id: RoomMessageId(4),
        },
    );
    let encoded = serde_json::to_value(&bus).unwrap();
    assert!(encoded.get("orchestrator").is_some());
    let replayed: crate::bus::model::BusState = serde_json::from_value(encoded).unwrap();
    assert_eq!(
        replayed
            .orchestrator_state()
            .context(crate::bus::model::RoomId(3))
            .semantic_revision,
        1
    );
}

#[test]
fn room_orchestrator_core_closed_surface_advertises_only_runtime_implemented_tools() {
    assert_eq!(
        EXPECTED_TOOL_NAMES.to_vec(),
        vec![
            "inspect_work",
            "wait_for_change",
            "read_agent",
            "observe_permission_prompt",
            "read_workflow_draft",
            "read_content",
            "send_message",
            "propose_room_brief",
            "abandon_idle_request",
            "persist_workflow_draft",
            "promote_workflow_draft",
            "acquire_resource",
            "release_resource",
            "approve_permission_once",
        ]
    );
    let registry = OrchestratorToolRegistry;
    for removed in [
        "create_agent",
        "steer_active_work",
        "suspend_work",
        "resolve_queued_work",
        "update_coordination",
        "retire_participant",
        "replace_participant",
        "request_human",
        "read_codebase_outline",
        "read_artifact",
    ] {
        assert!(!registry
            .schemas()
            .iter()
            .any(|schema| schema.name == removed));
        assert!(matches!(
            registry.decode(ModelToolCall {
                name: removed.into(),
                arguments: json!({}),
            }),
            Err(ToolPolicyError::UnknownTool(_))
        ));
    }
    for retired in ["manage_agents", "request_human"] {
        assert!(serde_json::from_value::<Capability>(json!(retired)).is_err());
    }
    let persist = registry
        .schemas()
        .iter()
        .find(|schema| schema.name == "persist_workflow_draft")
        .unwrap()
        .provider_definition();
    assert_eq!(
        persist["function"]["parameters"]["required"],
        json!(["draft_id", "expected_revision", "workflow_id", "markdown"])
    );
}

#[test]
fn room_orchestrator_core_propose_room_brief_decodes_without_room_author_or_path() {
    let registry = OrchestratorToolRegistry;
    let arguments = json!({"expected_revision": 0, "goal": "Ship", "non_goals": "No coding"});
    assert_eq!(
        registry
            .decode(ModelToolCall {
                name: "propose_room_brief".into(),
                arguments: arguments.clone(),
            })
            .unwrap(),
        OrchestratorCommand::Operation(RoomOperation::ProposeRoomBrief {
            expected_revision: 0,
            goal: "Ship".into(),
            non_goals: "No coding".into(),
        })
    );
    for forged in ["room_id", "author", "path", "revision", "locked"] {
        let mut forged_arguments = arguments.clone();
        forged_arguments[forged] = json!(1);
        assert!(matches!(
            registry.decode(ModelToolCall {
                name: "propose_room_brief".into(),
                arguments: forged_arguments,
            }),
            Err(ToolPolicyError::InvalidArguments(_))
        ));
    }
    let schema = registry
        .schemas()
        .iter()
        .find(|schema| schema.name == "propose_room_brief")
        .unwrap();
    assert!(schema.mutating);
    assert_eq!(
        schema.provider_definition()["function"]["parameters"]["required"],
        json!(["expected_revision", "goal", "non_goals"])
    );
}
