use super::{callback, callbacks, order_records, AgentId, CallbackEventKind, ProviderKind};
use serde_json::{json, Value};
use std::path::PathBuf;

fn record(sequence: u64, provider: ProviderKind, value: Value) -> callbacks::Record {
    callbacks::Record {
        id: format!("record-{sequence}"),
        sequence,
        at_ms: 20 + sequence,
        manifest: callbacks::Manifest {
            routing_key: callbacks::RoutingKey(7),
            provider,
            launch_id: "launch".into(),
        },
        value,
        reporter: Vec::new(),
        nested_provider: false,
    }
}

#[test]
fn neutral_record_mapping_keeps_validated_owner_and_original_callback_metadata() {
    let record = record(3, ProviderKind::ClaudeCode, json!({}));
    let mapped = callback(
        &record,
        AgentId(7),
        "session".into(),
        "prompt".into(),
        Some("payload".into()),
        CallbackEventKind::PromptStarted,
    );
    assert_eq!(mapped.agent_id, AgentId(7));
    assert_eq!(mapped.callback_id, "record-3");
    assert_eq!(mapped.sequence, 3);
    assert_eq!(mapped.occurred_at_ms, 23);
    assert_eq!(mapped.launch_id, "launch");
    assert_eq!(mapped.provider_session_id.as_deref(), Some("session"));
    assert_eq!(mapped.provider_turn_id.as_deref(), Some("prompt"));
    assert_eq!(mapped.provider_prompt_id.as_deref(), Some("prompt"));
    assert_eq!(mapped.prompt_payload.as_deref(), Some("payload"));
}

#[test]
fn non_claude_record_mapping_does_not_invent_a_provider_prompt_id() {
    for provider in [ProviderKind::Codex, ProviderKind::Cursor] {
        let mapped = callback(
            &record(1, provider, json!({})),
            AgentId(7),
            "session".into(),
            "turn".into(),
            None,
            CallbackEventKind::Final {
                text: "reply".into(),
            },
        );
        assert_eq!(mapped.provider_turn_id.as_deref(), Some("turn"));
        assert_eq!(mapped.provider_prompt_id, None);
        assert_eq!(mapped.prompt_payload, None);
    }
}

#[test]
fn neutral_turn_order_keeps_session_then_submit_before_companion_finals() {
    let events = [
        (
            1,
            json!({"hook_event_name":"Stop","session_id":"s","prompt_id":"first","last_assistant_message":"first reply"}),
        ),
        (
            2,
            json!({"hook_event_name":"Stop","session_id":"s","prompt_id":"second","last_assistant_message":"second reply"}),
        ),
        (
            3,
            json!({"hook_event_name":"UserPromptSubmit","session_id":"s","prompt_id":"first","prompt":"first"}),
        ),
        (
            4,
            json!({"hook_event_name":"SessionStart","session_id":"s"}),
        ),
        (
            5,
            json!({"hook_event_name":"UserPromptSubmit","session_id":"s","prompt_id":"second","prompt":"second"}),
        ),
    ];
    let mut records = events
        .into_iter()
        .map(|(sequence, value)| {
            (
                PathBuf::from(format!("event-{sequence}.json")),
                record(sequence, ProviderKind::ClaudeCode, value),
            )
        })
        .collect();
    order_records(&mut records);
    assert_eq!(
        records
            .iter()
            .map(|(_, record)| record.sequence)
            .collect::<Vec<_>>(),
        [4, 3, 1, 5, 2]
    );
}

mod orchestrator_turn_routing {
    use super::super::super::*;
    use crate::messaging::native::TransportError;
    use serde_json::json;

    struct Accepting;
    impl Transport for Accepting {
        fn request(&mut self, _method: Method) -> Result<ResponseResult, TransportError> {
            Ok(ResponseResult::Ok {})
        }
    }

    struct Bus {
        worker: Worker,
        master: RoomId,
        room: RoomId,
        orchestrator: AgentId,
        dir: PathBuf,
    }

    impl Drop for Bus {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn bus() -> Bus {
        let dir = crate::utils::test_temp::unique_temp_path("bus-routing");
        let mut worker = Worker::open(dir.clone(), Box::new(Accepting)).unwrap();
        let mut state = worker.state.clone();
        let master = state.master_room().unwrap().id;
        let room = state.create_room("work").unwrap();
        let orchestrator = state
            .create_agent(master, "orch", Provider::ClaudeCode, dir.clone(), None)
            .unwrap();
        state.bind_orchestrator(orchestrator, room).unwrap();
        state
            .set_agent_runtime_identity(
                orchestrator,
                AgentRuntimeIdentity {
                    launch_id: Some("orch".into()),
                    terminal_id: Some("terminal-orch".into()),
                    pane_id: Some("pane-orch".into()),
                    session_id: Some("session-orch".into()),
                },
            )
            .unwrap();
        state.confirm_hook_setup(orchestrator).unwrap();
        state
            .observe_status(orchestrator, RuntimeStatus::Idle, 1)
            .unwrap();
        callbacks::initialize(
            &dir.join("callbacks/orch"),
            &callbacks::Manifest {
                routing_key: callbacks::RoutingKey(orchestrator.0),
                provider: Provider::ClaudeCode,
                launch_id: "orch".into(),
            },
        )
        .unwrap();
        worker.save(state).unwrap();
        Bus {
            worker,
            master,
            room,
            orchestrator,
            dir,
        }
    }

    impl Bus {
        fn hooks(&mut self, values: &[serde_json::Value]) {
            let spool = self.dir.join("callbacks/orch");
            for value in values {
                callbacks::append(
                    &spool,
                    "orch",
                    Provider::ClaudeCode,
                    value.clone(),
                    &[],
                    false,
                )
                .unwrap();
            }
            self.worker
                .consume_callbacks(self.orchestrator, &spool)
                .unwrap();
        }

        fn notices(&self, room: RoomId) -> Vec<String> {
            self.worker
                .state
                .room(room)
                .unwrap()
                .notices
                .iter()
                .map(|notice| notice.text.clone())
                .collect()
        }
    }

    #[test]
    fn later_uncontended_hooks_preserve_staged_turn_report_order() {
        let mut bus = bus();
        let spool = bus.dir.join("callbacks/orch");
        let lease = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(spool.join("append.lock"))
            .unwrap();
        lease.lock().unwrap();
        for value in [
            json!({"hook_event_name":"UserPromptSubmit","session_id":"session-orch","prompt_id":"first","prompt":"first background turn"}),
            json!({"hook_event_name":"Stop","session_id":"session-orch","prompt_id":"first","last_assistant_message":"First completed report."}),
        ] {
            callbacks::append(&spool, "orch", Provider::ClaudeCode, value, &[], false).unwrap();
        }
        drop(lease);
        // The next turn begins after the earlier hooks returned successfully.
        // A busy poller has not yet drained their durable staged envelopes.
        bus.hooks(&[
            json!({"hook_event_name":"UserPromptSubmit","session_id":"session-orch","prompt_id":"second","prompt":"second background turn"}),
            json!({"hook_event_name":"Stop","session_id":"session-orch","prompt_id":"second","last_assistant_message":"Second completed report."}),
        ]);
        assert_eq!(
            bus.notices(bus.room),
            ["First completed report.", "Second completed report."],
            "publication after contention must preserve actual report order"
        );
    }

    #[test]
    fn a_background_turn_while_a_developer_message_waits_reports_in_the_work_room_and_the_developer_reply_stays_in_master(
    ) {
        let mut bus = bus();
        let (master, orchestrator) = (bus.master, bus.orchestrator);
        // A background command woke the orchestrator; its turn is running.
        bus.hooks(&[json!({"hook_event_name":"UserPromptSubmit","session_id":"session-orch","prompt_id":"bg","prompt":"<task-notification>"})]);
        // The developer writes in MASTER meanwhile; Bus types it.
        let mut state = bus.worker.state.clone();
        let request = state
            .submit_message_from(
                master,
                Draft {
                    text: "status?".into(),
                    files: Vec::new(),
                    recipient_ids: [orchestrator].into(),
                },
                Author::Developer,
                crate::messaging::storage::io::now_ms(),
            )
            .unwrap()[0];
        bus.worker.save(state).unwrap();
        bus.worker.submit_ready().unwrap();
        let typed = {
            let request = bus.worker.state.request(request).unwrap();
            request
                .submitted_payload
                .clone()
                .unwrap_or_else(|| request.prompt.rendered_payload())
        };
        // The background turn ends; then the developer's message runs.
        bus.hooks(&[
            json!({"hook_event_name":"Stop","session_id":"session-orch","prompt_id":"bg","last_assistant_message":"Gate passed."}),
            json!({"hook_event_name":"UserPromptSubmit","session_id":"session-orch","prompt_id":"dev","prompt":typed}),
            json!({"hook_event_name":"Stop","session_id":"session-orch","prompt_id":"dev","last_assistant_message":"All green; merging."}),
        ]);
        assert_eq!(bus.notices(bus.room), ["Gate passed."]);
        assert!(bus.notices(master).is_empty(), "{:?}", bus.notices(master));
        let request = bus.worker.state.request(request).unwrap();
        assert_eq!(
            request.reply().map(|f| f.text.as_str()),
            Some("All green; merging."),
            "{request:?}"
        );
    }
}
