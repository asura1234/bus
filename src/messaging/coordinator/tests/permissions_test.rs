use super::*;

#[test]
fn permission_handler_unreadable_fallback_reaches_work_room_orchestrator() {
    let (mut worker, agent, room, orchestrator, _, dir) = worker(true);
    post_blocked(&mut worker.state, agent, false).unwrap();
    assert_eq!(
        sent_by(&worker, room, agent)[0].0,
        vec![orchestrator.unwrap()]
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn permission_handler_every_blocked_episode_uses_service_without_classification() {
    for readable in [true, false] {
        let (mut worker, agent, room, _, _, dir) = worker(true);
        let handler = worker
            .state
            .create_agent(room, "service", Provider::ClaudeCode, dir.clone(), None)
            .unwrap();
        let mut value = serde_json::to_value(&worker.state).unwrap();
        value["agents"][handler.0.to_string()]["permission_handler"] = json!(true);
        worker.state = serde_json::from_value(value).unwrap();
        worker.state.confirm_hook_setup(handler).unwrap();
        worker
            .state
            .set_agent_runtime_identity(
                handler,
                AgentRuntimeIdentity {
                    launch_id: Some("handler-launch".into()),
                    terminal_id: Some("handler-terminal".into()),
                    pane_id: Some("handler-pane".into()),
                    session_id: Some("handler-session".into()),
                },
            )
            .unwrap();
        worker
            .state
            .observe_status(handler, RuntimeStatus::Idle, 1)
            .unwrap();
        post_blocked(&mut worker.state, agent, readable).unwrap();
        assert_eq!(sent_by(&worker, room, agent)[0].0, vec![handler]);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

fn ready_permission_handler(worker: &mut Worker, room: RoomId, dir: &std::path::Path) -> AgentId {
    let handler = worker
        .state
        .create_agent(
            room,
            "service",
            Provider::ClaudeCode,
            dir.to_path_buf(),
            None,
        )
        .unwrap();
    worker.state.mark_permission_handler(handler).unwrap();
    worker.state.confirm_hook_setup(handler).unwrap();
    worker
        .state
        .set_agent_runtime_identity(
            handler,
            AgentRuntimeIdentity {
                launch_id: Some("handler-launch".into()),
                terminal_id: Some("handler-terminal".into()),
                pane_id: Some("handler-pane".into()),
                session_id: Some("handler-session".into()),
            },
        )
        .unwrap();
    worker
        .state
        .observe_status(handler, RuntimeStatus::Idle, 1)
        .unwrap();
    handler
}
fn assignment_params(
    worker: &Worker,
    agent: AgentId,
    handler: AgentId,
    fingerprint: &str,
) -> serde_json::Value {
    json!({"agent":agent.0.to_string(),"handler":handler.0.to_string(),"assignment":worker.state.permission_assignment(agent).unwrap().token,"fingerprint":fingerprint})
}
#[test]
fn permission_handler_safe_approval_requires_confirmed_native_answer_and_keeps_audit() {
    let (mut worker, agent, room, _, screen, dir) = worker(true);
    let handler = ready_permission_handler(&mut worker, room, &dir);
    screen.lock().unwrap().dialog = true;
    notify_steady(
        &mut worker,
        agent,
        Some(&screen.lock().unwrap().current_dialog().unwrap().id),
    );
    let fp = worker.observe_dialog(agent).unwrap()["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut params = assignment_params(&worker, agent, handler, &fp);
    params["outcome"] = json!("approved");
    params["summary"] = json!("cargo test in own worktree; standing safe policy");
    assert!(
        permission_control(&mut worker, "agent.permission-decision", &params).is_err(),
        "a textual claim alone cannot approve"
    );
    params["option"] = json!("1");
    params.as_object_mut().unwrap().remove("outcome");
    params.as_object_mut().unwrap().remove("summary");
    assert_eq!(
        permission_control(&mut worker, "agent.dialog.choose", &params).unwrap()["outcome"],
        "closed"
    );
    assert!(!screen.lock().unwrap().dialog);
    params.as_object_mut().unwrap().remove("option");
    params["outcome"] = json!("approved");
    params["summary"] = json!("own-worktree tests; safe policy");
    permission_control(&mut worker, "agent.permission-decision", &params).unwrap();
    assert!(
        worker
            .state
            .agent(handler)
            .unwrap()
            .permission_reset_required
    );
    assert_eq!(sent_by(&worker, room, handler)[0].0, vec![]);
    let restored: BusState =
        serde_json::from_slice(&serde_json::to_vec(&worker.state).unwrap()).unwrap();
    assert_eq!(restored.permission_decisions().len(), 1);
    assert_eq!(
        restored.permission_decisions()[0].assignment.outcome,
        Some(crate::messaging::model::PermissionOutcome::Approved)
    );
    assert!(permission_control(&mut worker, "agent.permission-decision", &params).is_err());
    let public_state = permission_control(&mut worker, "state", &json!({})).unwrap();
    assert!(
        !public_state.to_string().contains(&fp),
        "routine state inspection must not expose encoded raw dialog claims"
    );
    assert!(public_state["permission_decisions"][0]["fingerprint"].is_string());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn permission_handler_escalated_and_routed_decisions_never_answer_and_stay_in_room() {
    for outcome in ["escalated", "routed"] {
        let (mut worker, agent, room, orchestrator, screen, dir) = worker(true);
        let handler = ready_permission_handler(&mut worker, room, &dir);
        screen.lock().unwrap().dialog = true;
        screen.lock().unwrap().question = outcome == "routed";
        let episode = screen.lock().unwrap().current_dialog().unwrap().id;
        notify_steady(&mut worker, agent, Some(&episode));
        let mut p = assignment_params(&worker, agent, handler, "unavailable");
        p["outcome"] = json!(outcome);
        p["summary"] = json!(if outcome == "routed" {
            "question requires orchestrator choice"
        } else {
            "destructive outside roots; manual authority required"
        });
        permission_control(&mut worker, "agent.permission-decision", &p).unwrap();
        assert!(
            screen.lock().unwrap().dialog,
            "decision must not send native keys"
        );
        let notices = sent_by(&worker, room, handler);
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].0, vec![orchestrator.unwrap()]);
        let raw = worker
            .state
            .requests()
            .find(|r| r.agent_id == orchestrator.unwrap())
            .unwrap()
            .prompt
            .text
            .clone();
        assert!(!raw.contains('\n'));
        assert!(!raw.contains(&episode));
        assert_bus_sent_nothing(&worker);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
#[test]
fn permission_handler_fallback_rejects_stale_completion_and_answer_once() {
    let (mut worker, agent, room, orchestrator, screen, dir) = worker(true);
    let handler = ready_permission_handler(&mut worker, room, &dir);
    screen.lock().unwrap().dialog = true;
    let episode = screen.lock().unwrap().current_dialog().unwrap().id;
    notify_steady(&mut worker, agent, Some(&episode));
    let fp = worker.observe_dialog(agent).unwrap()["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut p = assignment_params(&worker, agent, handler, &fp);
    p["option"] = json!("1");
    worker
        .state
        .observe_status(
            handler,
            RuntimeStatus::Unavailable,
            crate::messaging::storage::io::now_ms(),
        )
        .unwrap();
    worker.maintain_permissions().unwrap();
    worker.maintain_permissions().unwrap();
    assert!(permission_control(&mut worker, "agent.dialog.choose", &p).is_err());
    assert!(screen.lock().unwrap().dialog);
    p.as_object_mut().unwrap().remove("option");
    p["outcome"] = json!("routed");
    p["summary"] = json!("late decision");
    assert!(permission_control(&mut worker, "agent.permission-decision", &p).is_err());
    assert_eq!(
        sent_by(&worker, room, handler)
            .iter()
            .filter(|(to, _)| to == &vec![orchestrator.unwrap()])
            .count(),
        1
    );
    assert_eq!(worker.state.permission_decisions().len(), 1);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn permission_handler_new_screen_cannot_inherit_old_assignment_answer() {
    let (mut worker, agent, room, _, screen, dir) = worker(true);
    let handler = ready_permission_handler(&mut worker, room, &dir);
    screen.lock().unwrap().dialog = true;
    let episode = screen.lock().unwrap().current_dialog().unwrap().id;
    notify_steady(&mut worker, agent, Some(&episode));
    screen.lock().unwrap().text =
        Some("Dangerous NEW command, old assignment must not authorize this".into());
    let fp = worker.observe_dialog(agent).unwrap()["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut p = assignment_params(&worker, agent, handler, &fp);
    p["option"] = json!("1");
    assert!(
        permission_control(&mut worker, "agent.dialog.choose", &p).is_err(),
        "a new fingerprint from a new screen is not authority from the old lease"
    );
    assert!(screen.lock().unwrap().dialog);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn permission_handler_deadline_is_durable_and_expires_after_restart_once() {
    let (mut worker, agent, room, orchestrator, _, dir) = worker(true);
    let handler = ready_permission_handler(&mut worker, room, &dir);
    worker
        .state
        .dispatch_permission(agent, "unknown".into(), 1)
        .unwrap();
    worker.state = serde_json::from_slice(&serde_json::to_vec(&worker.state).unwrap()).unwrap();
    assert!(worker
        .state
        .validate_permission_lease(
            agent,
            handler,
            &worker.state.permission_assignment(agent).unwrap().token,
            90_001
        )
        .is_err());
    worker.maintain_permissions().unwrap();
    worker.maintain_permissions().unwrap();
    assert_eq!(worker.state.permission_decisions().len(), 1);
    assert_eq!(
        sent_by(&worker, room, handler)[0].0,
        vec![orchestrator.unwrap()]
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn permission_handler_policy_standing_safe_authority_and_protected_live_paths_remain_attached() {
    let dir = std::env::temp_dir().join(format!(
        "bus-handler-prompt-{}",
        crate::messaging::storage::io::now_ns()
    ));
    crate::messaging::storage::io::private_dir(&dir).unwrap();
    let text =
        crate::messaging::coordinator::permissions::handler_prompt(RoomId(12), AgentId(14), &dir);
    let path = crate::messaging::orchestration::write_prompt(&dir, &text).unwrap();
    let args =
        crate::messaging::orchestration::resume_prompt_args(Provider::ClaudeCode, &dir).unwrap();
    assert!(args.iter().any(|a| a.contains(path.to_str().unwrap())));
    for required in [
        "standing user authorization",
        "ALL shell segments",
        "multi-select",
        "NO ANSWER",
        "credentials",
        "NEVER grant broad permanent",
        "Protected running Bus executable",
        "/clear",
        "NEVER log in MASTER",
    ] {
        assert!(text.contains(required), "policy missing {required}");
    }
    std::fs::remove_dir_all(dir).unwrap();
}

fn permission_control(
    worker: &mut Worker,
    method: &str,
    params: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let request = crate::messaging::control::Request {
        id: format!("test-{}", crate::messaging::storage::io::now_ns()),
        method: method.into(),
        params: params.clone(),
    };
    let result = worker.control_response_with_events(&request, None);
    if result.ok {
        Ok(result.result)
    } else {
        Err(format!("{:?}", result.error))
    }
}
#[test]
fn permission_handler_fallback_withdraws_queued_assignment_before_reset() {
    let (mut worker, agent, room, _, _, dir) = worker(true);
    let handler = ready_permission_handler(&mut worker, room, &dir);
    worker
        .state
        .dispatch_permission(
            agent,
            "unknown".into(),
            crate::messaging::storage::io::now_ms(),
        )
        .unwrap();
    assert!(worker.state.next_queued_request(handler).is_some());
    worker
        .permission_fallback(agent, "delivery unavailable")
        .unwrap();
    assert!(
        worker.state.next_queued_request(handler).is_none(),
        "a terminal assignment must not remain queued and prevent /clear forever"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn permission_handler_clear_suspends_delivery_until_new_session_rebind_and_keeps_audit() {
    struct ClearOnly(Arc<Mutex<Vec<String>>>);
    impl Transport for ClearOnly {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            let Method::AgentPromptIfIdle(p) = method else {
                panic!("unexpected request {method:?}")
            };
            self.0.lock().unwrap().push(p.text);
            Ok(ResponseResult::AgentPrompted {agent:serde_json::from_value(json!({"terminal_id":"handler-terminal","agent":"claude","agent_status":"idle","workspace_id":"w","tab_id":"t","pane_id":"handler-pane","focused":false,"interactive_ready":true,"revision":1})).unwrap()})
        }
    }
    let (mut worker, agent, room, _, _, dir) = worker(true);
    let handler = ready_permission_handler(&mut worker, room, &dir);
    let other = worker
        .state
        .create_agent(room, "other", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    let now = crate::messaging::storage::io::now_ms();
    worker
        .state
        .dispatch_permission(agent, "unknown".into(), now)
        .unwrap();
    worker
        .state
        .complete_permission(
            agent,
            crate::messaging::model::PermissionOutcome::Routed,
            "unknown screen",
            now,
        )
        .unwrap();
    worker
        .state
        .dispatch_permission(other, "other-screen".into(), now)
        .unwrap();
    assert!(!worker.state.permission_assignment(other).unwrap().delivered);
    let clears = Arc::default();
    worker.transport = Box::new(ClearOnly(Arc::clone(&clears)));
    worker.maintain_permissions().unwrap();
    assert_eq!(*clears.lock().unwrap(), ["/clear"]);
    assert!(worker.state.agent(handler).unwrap().session_reset_pending);
    worker.maintain_permissions().unwrap();
    assert_eq!(clears.lock().unwrap().len(), 1);
    assert!(!worker.state.permission_assignment(other).unwrap().delivered);
    worker
        .state
        .rebind_reset_session(handler, "fresh-session".into())
        .unwrap();
    worker.maintain_permissions().unwrap();
    assert!(
        !worker
            .state
            .agent(handler)
            .unwrap()
            .permission_reset_required
    );
    assert!(worker.state.permission_assignment(other).unwrap().delivered);
    assert_eq!(worker.state.permission_decisions().len(), 1);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn permission_handler_uncertain_native_answer_falls_back_and_cannot_retry() {
    struct LostAnswer(FakeServer);
    impl Transport for LostAnswer {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            if matches!(method, Method::AgentDialogChoose(_)) {
                return Err(TransportError {
                    code: None,
                    message: "lost acknowledgement".into(),
                    definitely_rejected: false,
                });
            }
            self.0.request(method)
        }
    }
    let (mut worker, agent, room, orchestrator, screen, dir) = worker(true);
    let handler = ready_permission_handler(&mut worker, room, &dir);
    screen.lock().unwrap().dialog = true;
    let episode = screen.lock().unwrap().current_dialog().unwrap().id;
    notify_steady(&mut worker, agent, Some(&episode));
    let fp = worker.observe_dialog(agent).unwrap()["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut p = assignment_params(&worker, agent, handler, &fp);
    p["option"] = json!("1");
    worker.transport = Box::new(LostAnswer(FakeServer(screen.clone())));
    assert!(permission_control(&mut worker, "agent.dialog.choose", &p).is_err());
    assert!(worker
        .state
        .dialog_answer_spent(crate::messaging::model::fingerprint_digest(&fp)));
    assert!(
        worker
            .state
            .permission_assignment(agent)
            .unwrap()
            .answer_attempted
    );
    assert!(
        !worker
            .state
            .permission_assignment(agent)
            .unwrap()
            .answer_confirmed
    );
    assert!(permission_control(&mut worker, "agent.dialog.choose", &p).is_err());
    assert_eq!(
        sent_by(&worker, room, handler)[0].0,
        vec![orchestrator.unwrap()]
    );
    assert_eq!(worker.state.permission_decisions().len(), 1);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn permission_handler_broadcast_filters_service_and_reserves_managed_name() {
    let (mut worker, agent, room, _, _, dir) = worker(false);
    let handler = ready_permission_handler(&mut worker, room, &dir);
    let receipt = worker
        .control_send(&json!({"room":room.0.to_string(),"to":["all"],"text":"ordinary work"}))
        .unwrap();
    let message = PromptId(receipt["message_id"].as_u64().unwrap());
    let recipients: Vec<_> = worker
        .state
        .requests()
        .filter(|r| r.prompt.id == message)
        .map(|r| r.agent_id)
        .collect();
    assert_eq!(recipients, [agent]);
    assert!(!recipients.contains(&handler));
    assert_eq!(
        worker.state.create_agent(
            room,
            "Permission Handler",
            Provider::ClaudeCode,
            dir.clone(),
            None
        ),
        Err(ModelError::ReservedAgentName)
    );
    assert_eq!(
        worker.state.rename_agent(handler, "ordinary"),
        Err(ModelError::ReservedAgentName)
    );
    assert_eq!(worker.state.room_status(room), RuntimeStatus::Idle);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn permission_handler_master_orchestrator_and_own_blockers_use_work_room_without_recursion() {
    let (mut worker, _, room, orchestrator, _, dir) = worker(true);
    let orchestrator = orchestrator.unwrap();
    let handler = ready_permission_handler(&mut worker, room, &dir);
    notify_steady(&mut worker, orchestrator, Some(BLOCKED));
    assert_eq!(sent_by(&worker, room, orchestrator)[0].0, vec![handler]);
    worker
        .state
        .observe_status(
            handler,
            RuntimeStatus::Blocked,
            crate::messaging::storage::io::now_ms(),
        )
        .unwrap();
    notify_steady(&mut worker, handler, Some(BLOCKED));
    assert_eq!(sent_by(&worker, room, handler)[0].0, vec![orchestrator]);
    let master = worker.state.master_room().unwrap().id;
    assert!(worker.state.requests().all(|r| r.room_id != master));
    assert!(worker.state.room(master).unwrap().notices.is_empty());
    assert_eq!(
        worker
            .state
            .requests()
            .filter(|r| r.agent_id == handler)
            .count(),
        1,
        "handler's own blocker must not recursively target itself"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn permission_handler_launch_role_is_durable_before_native_creation_and_never_duplicated() {
    struct RefuseTab {
        state: PathBuf,
        calls: Arc<Mutex<usize>>,
    }
    impl Transport for RefuseTab {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            let Method::TabCreate(p) = method else {
                panic!("unexpected native operation {method:?}")
            };
            assert_eq!(p.label.as_deref(), Some("permission handler"));
            assert!(p.env.get("BUS_PERMISSION_HANDLER_ID").is_some());
            let state = crate::messaging::storage::state_store::JsonStore::new(self.state.clone())
                .load()
                .unwrap()
                .unwrap();
            assert_eq!(state.agents().filter(|a| a.permission_handler).count(), 1);
            *self.calls.lock().unwrap() += 1;
            Err(TransportError {
                code: None,
                message: "deliberate scratch launch refusal".into(),
                definitely_rejected: true,
            })
        }
    }
    let (mut worker, agent, room, _, _, dir) = worker(false);
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, 1)
        .unwrap();
    let calls = Arc::default();
    worker.transport = Box::new(RefuseTab {
        state: dir.join("state.json"),
        calls: Arc::clone(&calls),
    });
    worker.maintain_permission_handlers().unwrap();
    worker.maintain_permission_handlers().unwrap();
    assert_eq!(*calls.lock().unwrap(), 1);
    let handler = worker.state.permission_handler_of(room).unwrap();
    assert_eq!(handler.name, "permission handler");
    assert!(handler.actionable_error.is_some());
    assert!(handler
        .cwd
        .starts_with(dir.join("permission-handlers").canonicalize().unwrap()));
    let spool = dir
        .join("callbacks")
        .join(handler.runtime_identity.launch_id.as_ref().unwrap());
    assert!(spool
        .join(crate::messaging::orchestration::PROMPT_FILE)
        .is_file());
    assert!(
        !crate::messaging::orchestration::resume_prompt_args(Provider::ClaudeCode, &spool)
            .unwrap()
            .is_empty()
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn permission_handler_room_deletion_retires_pending_assignment_without_breaking_ticks() {
    let (mut worker, agent, room, _, _, dir) = worker(true);
    ready_permission_handler(&mut worker, room, &dir);
    worker
        .state
        .dispatch_permission(
            agent,
            "unknown".into(),
            crate::messaging::storage::io::now_ms(),
        )
        .unwrap();
    worker.state.delete_room(room).unwrap();
    assert!(
        worker.maintain_permissions().is_ok(),
        "a deleted room cannot receive fallback, but must not block future coordinator ticks"
    );
    assert_eq!(worker.state.permission_assignments().count(), 0);
    assert_eq!(worker.state.permission_decisions().len(), 1);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn permission_handler_public_state_bounds_recent_audit_and_hides_completed_assignments() {
    let (mut worker, agent, room, _, _, dir) = worker(true);
    ready_permission_handler(&mut worker, room, &dir);
    let now = crate::messaging::storage::io::now_ms();
    for offset in 0..105 {
        worker
            .state
            .dispatch_permission(agent, "unknown".into(), now + offset)
            .unwrap();
        worker
            .state
            .complete_permission(
                agent,
                crate::messaging::model::PermissionOutcome::Routed,
                "question requires orchestrator",
                now + offset,
            )
            .unwrap();
    }
    assert_eq!(worker.state.permission_decisions().len(), 100);
    let state = permission_control(&mut worker, "state", &json!({})).unwrap();
    assert_eq!(state["permission_decisions"].as_array().unwrap().len(), 10);
    assert!(state["permission_assignments"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(state["permission_decisions"][0].get("assignment").is_none());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn permission_profile_cwd_drift_keeps_frozen_root_in_assignment() {
    let (mut worker, agent, room, _, _, dir) = worker(true);
    ready_permission_handler(&mut worker, room, &dir);
    let original = worker
        .state
        .agent(agent)
        .unwrap()
        .cwd
        .canonicalize()
        .unwrap();
    let drift = dir.join("drift");
    std::fs::create_dir(&drift).unwrap();
    worker
        .state
        .update_agent_runtime_metadata(agent, drift.clone(), None)
        .unwrap();
    let value = serde_json::to_value(worker.state.agent(agent).unwrap()).unwrap();
    assert_eq!(value["launch_cwd"], json!(original));
    post_blocked(&mut worker.state, agent, true).unwrap();
    let prompt = &sent_by(&worker, room, agent)[0].1;
    assert!(
        prompt.contains(&format!(
            "Frozen authorized worker root: {}",
            original.display()
        )),
        "{prompt}"
    );
    assert!(prompt.contains(&format!("Actual observed cwd: {}", drift.display())));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn permission_profile_legacy_unknown_root_requires_escalation() {
    let (mut worker, agent, room, _, _, dir) = worker(true);
    ready_permission_handler(&mut worker, room, &dir);
    let mut value = serde_json::to_value(&worker.state).unwrap();
    value["agents"][agent.0.to_string()]
        .as_object_mut()
        .unwrap()
        .remove("launch_cwd");
    worker.state = serde_json::from_value(value).unwrap();
    post_blocked(&mut worker.state, agent, true).unwrap();
    assert!(sent_by(&worker, room, agent)[0]
        .1
        .contains("unknown (legacy); escalate"));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn permission_profile_service_pins_current_binary_and_narrow_operational_rules() {
    struct InspectService;
    impl Transport for InspectService {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            let Method::TabCreate(p) = method else {
                panic!("unexpected {method:?}")
            };
            let binary = std::env::current_exe().unwrap();
            let first = std::env::split_paths(std::ffi::OsStr::new(
                p.env.get("PATH").expect("service PATH"),
            ))
            .next()
            .unwrap();
            assert_eq!(first, binary.parent().unwrap());
            let settings: serde_json::Value = serde_json::from_slice(
                &std::fs::read(
                    PathBuf::from(&p.env["BUS_CALLBACK_DIR"]).join("claude-settings.json"),
                )
                .unwrap(),
            )
            .unwrap();
            let allow = settings["permissions"]["allow"].as_array().unwrap();
            for family in [
                "state",
                "agent dialog",
                "agent choose",
                "agent answer",
                "agent permission-decision",
            ] {
                assert!(
                    allow.iter().any(|v| v
                        .as_str()
                        .is_some_and(|s| s.contains(&format!("{family} *"))
                            && s.contains(&*binary.to_string_lossy()))),
                    "missing {family}: {allow:?}"
                );
            }
            assert!(allow
                .iter()
                .all(|v| !["Bash", "Bash(bus *)", "Bash(cargo *)", "Bash(rm *)"]
                    .contains(&v.as_str().unwrap())));
            assert!(settings["sandbox"].get("enabled").is_none());
            assert!(settings["sandbox"].get("filesystem").is_none());
            let prompt = std::fs::read_to_string(
                PathBuf::from(&p.env["BUS_CALLBACK_DIR"]).join("system-prompt.md"),
            )
            .unwrap();
            assert!(prompt.contains("Operational CLI prefix:"));
            let prefix = crate::agents::providers::launch::profile::cli_prefix(&binary);
            assert!(
                prompt.contains(&format!("{prefix} state")),
                "policy CLI must match permission matcher literally: {prompt}"
            );
            Err(TransportError {
                code: None,
                message: "inspected service".into(),
                definitely_rejected: true,
            })
        }
    }
    let (mut worker, _, _, _, _, dir) = worker(false);
    worker.transport = Box::new(InspectService);
    worker.maintain_permission_handlers().unwrap();
    assert_eq!(
        worker
            .state
            .agents()
            .filter(|a| a.permission_handler)
            .count(),
        1
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn permission_profile_assignment_uses_exact_current_executable_commands() {
    let (mut worker, agent, room, _, _, dir) = worker(true);
    ready_permission_handler(&mut worker, room, &dir);
    post_blocked(&mut worker.state, agent, true).unwrap();
    let prefix =
        crate::agents::providers::launch::profile::cli_prefix(&std::env::current_exe().unwrap());
    let prompt = &sent_by(&worker, room, agent)[0].1;
    assert!(
        prompt.contains(&format!("{prefix} agent dialog {}", agent.0)),
        "{prompt}"
    );
    assert!(prompt.contains(&format!("{prefix} agent permission-decision {}", agent.0)));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
