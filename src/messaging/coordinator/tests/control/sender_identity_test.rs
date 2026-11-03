use super::*;

fn setup() -> (Worker, RoomId, AgentId, AgentId, PathBuf) {
    let (mut w, room, sender, dir) = fixture();
    w.state
        .set_agent_runtime_identity(
            sender,
            AgentRuntimeIdentity {
                launch_id: Some("owner-launch".into()),
                pane_id: Some("w1:p2".into()),
                terminal_id: Some("term_internal".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let master = w.state.master_room().unwrap().id;
    let orch = w
        .state
        .create_agent(master, "room-orch", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    w.state.bind_orchestrator(orch, room).unwrap();
    w.transport = Box::new(Located {
        pane: Some(owned_pane_info()),
        agent_owned: true,
    });
    (w, room, sender, orch, dir)
}

fn caller() -> serde_json::Value {
    let mut caller = process_caller();
    caller["launch_id"] = json!("owner-launch");
    caller["pane_id"] = json!("w1:p2");
    caller["agent_context"] = json!(true);
    caller
}

#[test]
fn sender_identity_omitted_as_uses_agent_launch_after_rename_and_orchestrator_alias() {
    let (mut w, room, sender, orch, dir) = setup();
    for rename in [false, true] {
        if rename {
            w.state.rename_agent(sender, "renamed-owner").unwrap();
        }
        let sent = w.control_send(&json!({"room":room.0.to_string(),"to":["orchestrator"],"text":"probe window","caller":caller()}));
        assert!(sent.is_ok(), "{sent:?}");
        assert_eq!(sent.unwrap()["author_id"], json!(sender));
        let request = w.state.requests().last().unwrap();
        assert_eq!(request.prompt.author, Author::Agent(sender));
        assert_eq!(request.agent_id, orch);
        assert_eq!(w.store.load().unwrap().unwrap(), w.state);
    }
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn sender_identity_unknown_or_stale_context_refuses_without_mutation() {
    let (mut w, room, _, _, dir) = setup();
    struct Refusing;
    impl Transport for Refusing {
        fn request(&mut self, _: Method) -> Result<ResponseResult, TransportError> {
            Err(TransportError {
                message: "no such pane".into(),
                code: None,
                definitely_rejected: true,
            })
        }
    }
    w.transport = Box::new(Refusing);
    for caller in [
        serde_json::Value::Null,
        json!({"launch_id":"deleted-launch","pane_id":"w1:p2","agent_context":true}),
        json!({"pane_id":"gone-pane","process":crate::platform::process_instance(std::process::id()).unwrap()}),
        json!({"agent_context":true}),
        json!({}),
        process_caller(),
    ] {
        let before = w.state.clone();
        let sent = w.control_send(&json!({"room":room.0.to_string(),"to":["orchestrator"],"text":"ambiguous sender","caller":caller}));
        assert_eq!(w.state, before, "unresolved caller changed state: {sent:?}");
        assert!(sent.unwrap_err().contains("pass --as <your name or id>"));
    }
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}

fn process_caller() -> serde_json::Value {
    json!({"process":crate::platform::process_instance(std::process::id()).unwrap()})
}

struct Located {
    pane: Option<schema::PaneInfo>,
    agent_owned: bool,
}
impl Transport for Located {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        let Method::PaneLocateProcess(_) = method else {
            panic!("unexpected native method")
        };
        Ok(ResponseResult::CallerLocation {
            location: schema::CallerLocation {
                pane: self.pane.clone(),
                agent_owned: self.agent_owned,
            },
        })
    }
}

#[test]
fn sender_identity_developer_in_a_verified_plain_bus_shell_is_preserved() {
    let (mut w, room, _, _, dir) = setup();
    let mut pane = owned_pane_info();
    pane.pane_id = "w1:plain".into();
    w.transport = Box::new(Located {
        pane: Some(pane),
        agent_owned: false,
    });
    let mut caller = process_caller();
    caller["pane_id"] = json!("w1:plain");
    caller["bus_context"] = json!(true);
    let sent=w.control_send(&json!({"room":room.0.to_string(),"to":["orchestrator"],"text":"developer shell","caller":caller}));
    assert!(sent.is_ok(), "{sent:?}");
    assert_eq!(
        w.state.requests().last().unwrap().prompt.author,
        Author::Developer
    );
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn sender_identity_process_tree_infers_agent_and_refuses_a_deleted_owner() {
    let (mut w, room, sender, _, dir) = setup();
    w.transport = Box::new(Located {
        pane: Some(owned_pane_info()),
        agent_owned: true,
    });
    let params = json!({"room":room.0.to_string(),"to":["orchestrator"],"text":"process caller","caller":process_caller()});
    assert!(w.control_send(&params).is_ok());
    assert_eq!(
        w.state.requests().last().unwrap().prompt.author,
        Author::Agent(sender)
    );
    w.state.delete_agent(sender, 1).unwrap();
    let before = w.state.clone();
    let error = w.control_send(&params).unwrap_err();
    assert!(error.contains("pass --as <your name or id>"), "{error}");
    assert_eq!(w.state, before);
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn sender_identity_explicit_as_and_external_developer_keep_their_authors() {
    let (mut w, room, sender, _, dir) = setup();
    w.transport = Box::new(Located {
        pane: None,
        agent_owned: false,
    });
    for (explicit, expected) in [(true, Author::Agent(sender)), (false, Author::Developer)] {
        let mut params = json!({"room":room.0.to_string(),"to":["orchestrator"],"text":"hello","caller":process_caller()});
        if explicit {
            params["as"] = json!(sender.0.to_string());
            params["caller"] = serde_json::Value::Null;
        }
        assert!(w.control_send(&params).is_ok());
        assert_eq!(w.state.requests().last().unwrap().prompt.author, expected);
    }
    // The developer's TUI submits its draft directly, without CLI caller inference.
    w.state.set_draft_text(room, "TUI send").unwrap();
    w.state.set_draft_recipients(room, [sender]).unwrap();
    let id = w.state.submit_draft(room, 1).unwrap()[0];
    assert_eq!(
        w.state.request(id).unwrap().prompt.author,
        Author::Developer
    );
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn sender_identity_inferred_worker_cannot_bypass_master_guard() {
    let (mut w, _, _, orch, dir) = setup();
    let before = w.state.clone();
    let params = json!({"room":"master","to":[orch.0.to_string()],"text":"worker progress","caller":caller()});
    let direct = w.control_send(&params);
    assert_eq!(
        w.state, before,
        "inferred worker bypassed MASTER guard: {direct:?}"
    );
    assert!(direct.unwrap_err().contains("MASTER"));
    let sent = call(&mut w, "inferred-master", "message.send", params);
    assert!(!sent.ok, "{sent:?}");
    assert_eq!(sent.error.unwrap().code, "master_reserved");
    assert_eq!(w.state, before);
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn sender_identity_reply_to_infers_agent_without_as() {
    let (mut w, _, sender, orch, dir) = setup();
    let master = w.state.master_room().unwrap().id;
    let id = w
        .state
        .submit_message_from(
            master,
            Draft {
                text: "review".into(),
                files: vec![],
                recipient_ids: [sender].into(),
            },
            Author::Agent(orch),
            1,
        )
        .unwrap()[0];
    w.state.begin_submission(id, "owner-launch", 0).unwrap();
    let message = w.state.request(id).unwrap().prompt.id;
    let sent =
        w.control_send(&json!({"reply_to":message.0.to_string(),"text":"done","caller":caller()}));
    assert!(sent.is_ok(), "{sent:?}");
    assert_eq!(sent.unwrap()["stage"], "replied");
    assert_eq!(w.state.request(id).unwrap().room_id, master);
    assert_eq!(
        w.state
            .request(id)
            .unwrap()
            .completed_reply
            .as_ref()
            .unwrap()
            .content
            .text,
        "done"
    );
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn sender_identity_reported_no_as_deleted_name_never_becomes_a_developer_send() {
    let (mut w, room, _, _, dir) = setup();
    let deleted = w
        .state
        .create_agent(room, "cu-rev", Provider::Kimi, dir.clone(), None)
        .unwrap();
    w.state.delete_agent(deleted, 1).unwrap();
    let before = w.state.clone();
    let sent = w.control_send(&json!({"room":room.0.to_string(),"to":["cu-rev"],"text":"Granted exclusive target/probe window","caller":caller()}));
    assert_eq!(
        w.state, before,
        "the reported send was persisted or rerouted: {sent:?}"
    );
    let error = sent.unwrap_err();
    assert!(
        error.contains("cu-rev") && error.contains("deleted"),
        "{error}"
    );
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn sender_identity_deleted_and_unknown_recipients_never_reroute() {
    let (mut w, room, sender, _, dir) = setup();
    let deleted = w
        .state
        .create_agent(room, "cu-rev", Provider::Kimi, dir.clone(), None)
        .unwrap();
    w.state.delete_agent(deleted, 1).unwrap();
    for selector in [
        "cu-rev".to_owned(),
        deleted.0.to_string(),
        "never-existed".into(),
    ] {
        let before = w.state.clone();
        let sent = w.control_send(&json!({"room":room.0.to_string(),"as":sender.0.to_string(),"to":[selector],"text":"exclusive probe window"}));
        assert_eq!(w.state, before, "invalid recipient rerouted: {sent:?}");
        let error = sent.unwrap_err();
        assert!(error.contains(&selector), "{error}");
        if selector != "never-existed" {
            assert!(error.contains("deleted"), "{error}");
        }
    }
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn sender_identity_deleted_numeric_id_cannot_be_rebound_by_a_numeric_name() {
    let (mut w, room, sender, _, dir) = setup();
    let deleted = w
        .state
        .create_agent(room, "old-reviewer", Provider::Kimi, dir.clone(), None)
        .unwrap();
    w.state.delete_agent(deleted, 1).unwrap();
    w.state
        .create_agent(
            room,
            &deleted.0.to_string(),
            Provider::Kimi,
            dir.clone(),
            None,
        )
        .unwrap();
    let before = w.state.clone();
    let sent = w.control_send(&json!({"room":room.0.to_string(),"as":sender.0.to_string(),"to":[deleted.0.to_string()],"text":"old ID"}));
    assert_eq!(
        w.state, before,
        "deleted ID resolved to a numeric name: {sent:?}"
    );
    assert!(sent.unwrap_err().contains("deleted"));
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn sender_identity_request_id_replays_across_processes_only_for_the_same_author() {
    let (mut w, room, _, orch, dir) = setup();
    w.transport = Box::new(Located {
        pane: Some(owned_pane_info()),
        agent_owned: true,
    });
    let mut params = json!({"room":room.0.to_string(),"to":["orchestrator"],"text":"retry once","caller":process_caller()});
    let first = call(&mut w, "same-send", "message.send", params.clone());
    assert!(first.ok, "{first:?}");
    params["caller"]["process"]["pid"] = json!(12345);
    params["caller"]["process"]["birth"] = json!(67890);
    let retry = call(&mut w, "same-send", "message.send", params.clone());
    assert!(retry.ok, "{retry:?}");
    assert_eq!(retry.result, first.result);
    assert_eq!(w.state.requests().count(), 1);
    params["caller"] = process_caller();
    params["caller"]["launch_id"] = json!("orch-launch");
    w.state
        .set_agent_runtime_identity(
            orch,
            AgentRuntimeIdentity {
                launch_id: Some("orch-launch".into()),
                pane_id: Some("w1:orch".into()),
                terminal_id: Some("term_orch".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let mut pane = owned_pane_info();
    pane.pane_id = "w1:orch".into();
    pane.terminal_id = "term_orch".into();
    w.transport = Box::new(Located {
        pane: Some(pane),
        agent_owned: true,
    });
    let other = call(&mut w, "same-send", "message.send", params.clone());
    assert_eq!(other.error.unwrap().code, "id_conflict");
    params["text"] = json!("changed text");
    let changed = call(&mut w, "same-send", "message.send", params);
    assert_eq!(changed.error.unwrap().code, "id_conflict");
    assert_eq!(w.state.requests().count(), 1);
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn sender_identity_interrupt_follow_up_uses_the_recorded_developer_actor() {
    let (mut w, room, target, _, dir) = setup();
    w.state
        .observe_status(target, RuntimeStatus::Idle, 1)
        .unwrap();
    let receipt = call(
        &mut w,
        "developer-interrupt",
        "agent.interrupt",
        json!({"agent":target.0.to_string()}),
    );
    assert!(receipt.ok, "{receipt:?}");
    assert_eq!(receipt.result["author_id"], serde_json::Value::Null);
    let params = json!({"room":room.0.to_string(),"to":[target.0.to_string()],"text":"after interrupt","caller":{"interrupt_request_id":"developer-interrupt"}});
    let sent = call(&mut w, "interrupt-follow-up", "message.send", params);
    assert!(sent.ok, "{sent:?}");
    assert_eq!(
        w.state.requests().last().unwrap().prompt.author,
        Author::Developer
    );
    assert_eq!(w.store.load().unwrap().unwrap(), w.state);
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn sender_identity_interrupt_context_cannot_be_forged_or_retargeted() {
    let (mut w, room, target, orch, dir) = setup();
    w.state
        .observe_status(target, RuntimeStatus::Idle, 1)
        .unwrap();
    assert!(
        call(
            &mut w,
            "valid-interrupt",
            "agent.interrupt",
            json!({"agent":target.0.to_string()})
        )
        .ok
    );
    assert!(
        call(
            &mut w,
            "agent-interrupt",
            "agent.interrupt",
            json!({"agent":target.0.to_string(),"as":orch.0.to_string()})
        )
        .ok
    );
    assert!(
        !call(
            &mut w,
            "failed-interrupt",
            "agent.interrupt",
            json!({"agent":"missing"})
        )
        .ok
    );
    assert!(call(&mut w, "not-an-interrupt", "message.send", json!({"room":room.0.to_string(),"as":orch.0.to_string(),"to":[target.0.to_string()],"text":"another command"})).ok);
    for (proof, recipient) in [
        ("missing", target),
        ("failed-interrupt", target),
        ("not-an-interrupt", target),
        ("agent-interrupt", target),
        ("valid-interrupt", orch),
    ] {
        let before = w.state.clone();
        let sent = w.control_send(&json!({"room":room.0.to_string(),"to":[recipient.0.to_string()],"text":"untrusted follow-up","caller":{"interrupt_request_id":proof}}));
        assert!(sent.unwrap_err().contains("pass --as <your name or id>"));
        assert_eq!(w.state, before);
    }
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn sender_identity_exited_master_shell_does_not_reuse_its_inherited_launch() {
    let (mut w, room, receiver, orch, dir) = setup();
    w.state
        .set_agent_runtime_identity(receiver, AgentRuntimeIdentity::default())
        .unwrap();
    w.state
        .set_agent_runtime_identity(
            orch,
            AgentRuntimeIdentity {
                launch_id: Some("exited-orch-launch".into()),
                pane_id: Some("w1:p2".into()),
                terminal_id: Some("term_internal".into()),
                ..Default::default()
            },
        )
        .unwrap();
    for (agent_owned, exited) in [(false, false), (false, true), (true, true)] {
        let mut pane = owned_pane_info();
        pane.agent_process_exited = exited;
        w.transport = Box::new(Located {
            pane: Some(pane),
            agent_owned,
        });
        let mut caller = process_caller();
        caller["launch_id"] = json!("exited-orch-launch");
        caller["pane_id"] = json!("w1:p2");
        caller["agent_context"] = json!(true);
        let before = w.state.clone();
        let sent = w.control_send(&json!({"room":room.0.to_string(),"to":[receiver.0.to_string()],"text":"retained developer shell","caller":caller}));
        assert_eq!(
            w.state, before,
            "released native pane reused its old agent: {sent:?}"
        );
        assert!(sent.unwrap_err().contains("pass --as <your name or id>"));
    }
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn sender_identity_native_lookup_logs_the_refusal_cause_without_message_content() {
    struct FailedLookup;
    impl Transport for FailedLookup {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            assert!(matches!(method, Method::PaneLocateProcess(_)));
            Err(TransportError {
                code: Some("caller_unknown".into()),
                message: "stale process incarnation".into(),
                definitely_rejected: true,
            })
        }
    }
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        let (mut w, room, _, _, dir) = setup();
        w.transport = Box::new(FailedLookup);
        let before = w.state.clone();
        let sent = w.control_send(&json!({"room":room.0.to_string(),"to":["orchestrator"],"text":"PRIVATE_SEND_BODY","caller":process_caller()}));
        assert!(sent.unwrap_err().contains("pass --as <your name or id>"));
        assert_eq!(w.state, before);
        drop(w);
        std::fs::remove_dir_all(dir).unwrap();
    });
    let logs = capture.text();
    assert!(logs.contains("bus.send.caller_lookup_failed"), "{logs}");
    assert!(
        logs.contains("caller_unknown") && logs.contains("stale process incarnation"),
        "{logs}"
    );
    assert!(!logs.contains("PRIVATE_SEND_BODY"), "{logs}");
}
