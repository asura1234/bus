use super::launch_metrics_tests::{ledger, seed_launch};
use super::*;
use crate::messaging::model::{AgentStartup, LaunchOutcome, StartupRecovery};

struct ResumeNative {
    state_path: PathBuf,
    agent: AgentId,
    terminal: &'static str,
    pane: &'static str,
    started: bool,
    recovery: bool,
}
impl ResumeNative {
    fn info(&self, ready: bool) -> schema::AgentInfo {
        let s = JsonStore::new(self.state_path.clone())
            .load()
            .unwrap()
            .unwrap();
        let a = s.agent(self.agent).unwrap();
        let mut i = native_info(self.pane, "codex", "session");
        i.name = Some(crate::messaging::identity::managed_name(a.room_id, a.id));
        i.terminal_id = self.terminal.into();
        i.native_ready = ready;
        i.interactive_ready = ready;
        i.agent_status = if ready {
            schema::AgentStatus::Idle
        } else {
            schema::AgentStatus::Unknown
        };
        i
    }
    fn verify_before_input(&mut self) {
        assert!(
            !self.started,
            "one accepted attempt must not replay native input"
        );
        let s = JsonStore::new(self.state_path.clone())
            .load()
            .unwrap()
            .unwrap();
        let a = s
            .current_launch_attempt(self.agent)
            .expect("attempt durable before resume input");
        assert_eq!(s.launch_attempts().count(), 2);
        assert_eq!(a.outcome, LaunchOutcome::Pending);
        assert_eq!(a.terminal_id.as_deref(), Some(self.terminal));
        assert_eq!(
            a.kind,
            if self.recovery {
                crate::messaging::model::LaunchKind::Recovery
            } else {
                crate::messaging::model::LaunchKind::Resume
            }
        );
        if self.recovery {
            assert!(matches!(
                s.agent(self.agent)
                    .unwrap()
                    .startup
                    .as_ref()
                    .unwrap()
                    .recovery,
                Some(StartupRecovery::Requested)
            ));
        }
        self.started = true;
    }
}
impl Transport for ResumeNative {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        Ok(match method {
            Method::AgentList(_)|Method::AgentListWithDetectionContext(_)=>ResponseResult::AgentList {
                agents:if self.started || self.recovery {vec![self.info(self.started)]} else {vec![]}
            },
            Method::TabCreate(_)=>ResponseResult::TabCreated {
                tab:serde_json::from_value(json!({"tab_id":"t1","workspace_id":"w1","number":1,"label":"resume","focused":false,"pane_count":1,"agent_status":"unknown"})).unwrap(),
                root_pane:serde_json::from_value(json!({"pane_id":self.pane,"terminal_id":self.terminal,"workspace_id":"w1","tab_id":"t1","focused":false,"agent_status":"unknown","revision":1})).unwrap(),
            },
            Method::PaneGet(_)=>ResponseResult::PaneInfo {pane:serde_json::from_value(json!({"pane_id":self.pane,"terminal_id":self.terminal,"workspace_id":"w1","tab_id":"t1","focused":false,"agent_status":"unknown","revision":1,"agent_process_exited":false})).unwrap()},
            Method::AgentStart(_)|Method::AgentStartIfIdentity(_)=>{
                self.verify_before_input();
                ResponseResult::AgentStarted {agent:self.info(true),argv:vec!["codex".into()]}
            }
            _=>ResponseResult::Ok{},
        })
    }
}
fn prepare_resume(worker: &mut Worker, agent: AgentId, recovery: Option<StartupRecovery>) {
    seed_launch(worker, agent, u64::MAX);
    let mut state = worker.state.clone();
    if recovery.is_none() {
        let hooks = worker.data_dir.join(".codex/hooks.json");
        let document = crate::agents::providers::hook_json::merge_hooks(
            json!({}),
            crate::agents::providers::hook_json::HookContract::for_provider(Provider::Codex),
            &std::env::current_exe().unwrap(),
        )
        .unwrap();
        std::fs::create_dir_all(hooks.parent().unwrap()).unwrap();
        std::fs::write(hooks, serde_json::to_vec(&document).unwrap()).unwrap();
        super::super::launch_metrics::settle_current(
            &mut state,
            agent,
            LaunchOutcome::Failed,
            Some("command_failed"),
            2,
        );
        let mut identity = state.agent(agent).unwrap().runtime_identity.clone();
        identity.pane_id = None;
        state.set_agent_runtime_identity(agent, identity).unwrap();
    } else {
        state
            .set_agent_startup(
                agent,
                Some(AgentStartup {
                    command: "codex".into(),
                    started_at_ms: 1,
                    reported: true,
                    process_observed: false,
                    native_terminal_id: None,
                    recovery,
                    failure: None,
                }),
            )
            .unwrap();
        worker
            .missing_binary_checked
            .insert(agent, Some("terminal".into()));
    }
    state
        .observe_status(agent, RuntimeStatus::Unavailable, 1)
        .unwrap();
    worker.save(state).unwrap();
}
/// Pane close can report success while the previous terminal is still listed.
/// An uncertain tab create must not adopt that terminal as the new resume.
struct StaleResume {
    state_path: PathBuf,
    agent: AgentId,
    lists: u32,
}
impl StaleResume {
    fn ready(&self) -> schema::AgentInfo {
        let s = JsonStore::new(self.state_path.clone())
            .load()
            .unwrap()
            .unwrap();
        let a = s.agent(self.agent).unwrap();
        let mut info = native_info("pane", "codex", "session");
        info.name = Some(crate::messaging::identity::managed_name(a.room_id, a.id));
        info.terminal_id = "terminal".into();
        info.native_ready = true;
        info.interactive_ready = true;
        info.agent_status = schema::AgentStatus::Idle;
        info
    }
}
impl Transport for StaleResume {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::AgentList(_) | Method::AgentListWithDetectionContext(_) => {
                self.lists += 1;
                let agents = if self.lists == 1 {
                    Vec::new()
                } else {
                    vec![self.ready()]
                };
                Ok(ResponseResult::AgentList { agents })
            }
            Method::TabCreate(_) => Err(TransportError {
                code: None,
                message: "lost tab create".into(),
                definitely_rejected: false,
            }),
            _ => Ok(ResponseResult::Ok {}),
        }
    }
}

#[test]
fn launch_metrics_uncertain_resume_tab_does_not_adopt_the_previous_terminal() {
    let _providers = provider_binaries();
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    seed_launch(&mut worker, agent, u64::MAX);
    let hooks = worker.data_dir.join(".codex/hooks.json");
    let document = crate::agents::providers::hook_json::merge_hooks(
        json!({}),
        crate::agents::providers::hook_json::HookContract::for_provider(Provider::Codex),
        &std::env::current_exe().unwrap(),
    )
    .unwrap();
    std::fs::create_dir_all(hooks.parent().unwrap()).unwrap();
    std::fs::write(hooks, serde_json::to_vec(&document).unwrap()).unwrap();
    let mut state = worker.state.clone();
    state
        .observe_status(agent, RuntimeStatus::Unavailable, 1)
        .unwrap();
    worker.save(state).unwrap();
    worker.transport = Box::new(StaleResume {
        state_path: dir.join("state.json"),
        agent,
        lists: 0,
    });
    let result = worker.resume_room(room).unwrap();
    let reason = result["agents"][0]["reason"].as_str().unwrap_or("");
    assert!(
        reason.contains("Terminal creation outcome needs inspection"),
        "probe must stop on the uncertain tab create: {result}"
    );
    worker.poll().unwrap();
    let saved = ledger(&worker);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert_ne!(
        saved["2"]["outcome"], "usable",
        "unsent resume adopted the previous terminal: {saved}"
    );
    assert_ne!(saved["2"]["terminal_id"], "terminal", "{saved}");
}

#[test]
fn launch_metrics_uncertain_resume_expires_while_previous_terminal_stays_listed() {
    let _providers = provider_binaries();
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    seed_launch(&mut worker, agent, u64::MAX);
    let hooks = worker.data_dir.join(".codex/hooks.json");
    let document = crate::agents::providers::hook_json::merge_hooks(
        json!({}),
        crate::agents::providers::hook_json::HookContract::for_provider(Provider::Codex),
        &std::env::current_exe().unwrap(),
    )
    .unwrap();
    std::fs::create_dir_all(hooks.parent().unwrap()).unwrap();
    std::fs::write(hooks, serde_json::to_vec(&document).unwrap()).unwrap();
    let mut state = worker.state.clone();
    state
        .observe_status(agent, RuntimeStatus::Unavailable, 1)
        .unwrap();
    worker.save(state).unwrap();
    worker.transport = Box::new(StaleResume {
        state_path: dir.join("state.json"),
        agent,
        lists: 0,
    });
    let result = worker.resume_room(room).unwrap();
    let reason = result["agents"][0]["reason"].as_str().unwrap_or("");
    assert!(
        reason.contains("Terminal creation outcome needs inspection"),
        "probe must stop on the uncertain tab create: {result}"
    );
    worker.poll().unwrap();
    let listed = ledger(&worker);
    assert_eq!(listed["2"]["outcome"], "pending", "{listed}");
    assert!(listed["2"]["cause"].is_null(), "{listed}");
    assert!(listed["2"]["terminal_id"].is_null(), "{listed}");
    let mut doc = serde_json::to_value(&worker.state).unwrap();
    doc["launch_attempts"]["2"]["deadline_at_ms"] = json!(0);
    worker.save(serde_json::from_value(doc).unwrap()).unwrap();
    worker.poll().unwrap();
    let saved = ledger(&worker);
    let identity = worker
        .state
        .agent(agent)
        .unwrap()
        .runtime_identity
        .terminal_id
        .clone();
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(saved["2"]["outcome"], "failed", "{saved}");
    assert_eq!(saved["2"]["cause"], "startup_timeout", "{saved}");
    assert!(saved["2"]["terminal_id"].is_null(), "{saved}");
    assert_eq!(identity.as_deref(), Some("terminal"));
}

#[test]
fn launch_metrics_explicit_resume_records_before_input_and_preserves_failed_predecessor() {
    let _providers = provider_binaries();
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    prepare_resume(&mut worker, agent, None);
    worker.transport = Box::new(ResumeNative {
        state_path: dir.join("state.json"),
        agent,
        terminal: "new-terminal",
        pane: "new-pane",
        started: false,
        recovery: false,
    });
    let result = worker.resume_room(room).unwrap();
    assert_eq!(result["agents"][0]["outcome"], "resuming", "{result}");
    worker.poll().unwrap();
    let saved = ledger(&worker);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(saved.as_object().unwrap().len(), 2);
    assert_eq!(saved["1"]["outcome"], "failed");
    assert_eq!(saved["2"]["outcome"], "usable");
    assert_eq!(saved["2"]["kind"], "resume");
}
#[test]
fn launch_metrics_unsubmitted_recovery_is_a_new_attempt_on_the_same_terminal() {
    let _providers = provider_binaries();
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    prepare_resume(
        &mut worker,
        agent,
        Some(StartupRecovery::Unsubmitted { args: vec![] }),
    );
    worker.transport = Box::new(ResumeNative {
        state_path: dir.join("state.json"),
        agent,
        terminal: "terminal",
        pane: "pane",
        started: false,
        recovery: true,
    });
    let result = worker.resume_room(room).unwrap();
    assert_eq!(result["agents"][0]["outcome"], "resuming", "{result}");
    worker.poll().unwrap();
    let saved = ledger(&worker);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(saved.as_object().unwrap().len(), 2);
    assert_eq!(saved["2"]["kind"], "recovery");
    assert_eq!(saved["2"]["outcome"], "usable");
}
#[test]
fn launch_metrics_protected_unconfirmed_start_does_not_create_another_attempt() {
    let _providers = provider_binaries();
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    prepare_resume(&mut worker, agent, Some(StartupRecovery::Requested));
    worker.transport = Box::new(ResumeNative {
        state_path: dir.join("state.json"),
        agent,
        terminal: "terminal",
        pane: "pane",
        started: false,
        recovery: true,
    });
    let result = worker.resume_room(room).unwrap();
    let saved = ledger(&worker);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(result["agents"][0]["outcome"], "blocked");
    assert_eq!(saved.as_object().unwrap().len(), 1);
    assert_eq!(saved["1"]["outcome"], "pending");
}
#[test]
fn launch_metrics_native_restore_is_counted_once_across_repeat_polls_and_reopen() {
    let (mut worker, agent, _, dir, _) = fixture(Provider::Codex, vec![]);
    prepare_resume(&mut worker, agent, None);
    worker.transport = Box::new(ResumeNative {
        state_path: dir.join("state.json"),
        agent,
        terminal: "native-new",
        pane: "native-pane",
        started: true,
        recovery: false,
    });
    worker.poll().unwrap();
    worker.poll().unwrap();
    let before = ledger(&worker);
    drop(worker);
    let native = ResumeNative {
        state_path: dir.join("state.json"),
        agent,
        terminal: "native-new",
        pane: "native-pane",
        started: true,
        recovery: false,
    };
    let mut worker = Worker::open(dir.clone(), Box::new(native)).unwrap();
    worker.poll().unwrap();
    let after = ledger(&worker);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(before, after);
    assert_eq!(after.as_object().unwrap().len(), 2);
    assert_eq!(after["2"]["kind"], "native_resume");
    assert_eq!(after["2"]["outcome"], "usable");
}
#[test]
fn launch_metrics_legacy_reattach_does_not_invent_a_launch() {
    let (mut worker, agent, _, dir, _) = fixture(Provider::Codex, vec![]);
    worker.transport = Box::new(ResumeNative {
        state_path: dir.join("state.json"),
        agent,
        terminal: "terminal",
        pane: "pane",
        started: true,
        recovery: false,
    });
    worker.poll().unwrap();
    let saved = ledger(&worker);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert!(saved.as_object().unwrap().is_empty());
}

#[test]
fn launch_metrics_unsubmitted_recovery_preparation_failure_is_durable() {
    let (mut worker, agent, _, dir, calls) = fixture(Provider::Kimi, vec![]);
    prepare_resume(
        &mut worker,
        agent,
        Some(StartupRecovery::Unsubmitted { args: vec![] }),
    );
    assert_eq!(
        worker.state.agent(agent).unwrap().launch_attempt_id,
        Some(1),
        "the unsubmitted agent must still own its pending launch"
    );
    assert_eq!(ledger(&worker)["1"]["outcome"], "pending");
    let agent = worker.state.agent(agent).unwrap().clone();
    let result = worker.retry_unsubmitted_agent_start(&agent, vec![]);
    assert!(
        result
            .as_ref()
            .is_err_and(|error| error.contains("Bus resume capture configuration is missing")),
        "probe must reach saved executable preparation: {result:?}"
    );
    let current = worker.state.agent(agent.id).unwrap().launch_attempt_id;
    let saved = ledger(&worker);
    let sent = calls.lock().unwrap().clone();
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        sent.is_empty(),
        "preparation failure must not send a native retry: {sent:?}"
    );
    assert_eq!(saved.as_object().unwrap().len(), 2, "{saved}");
    assert_eq!(saved["1"]["outcome"], "pending", "{saved}");
    assert!(saved["1"]["cause"].is_null(), "{saved}");
    assert_eq!(current, Some(1), "{saved}");
    assert_eq!(saved["2"]["kind"], "recovery");
    assert_eq!(saved["2"]["outcome"], "failed");
    assert!(saved["2"]["agent_id"].is_null(), "{saved}");
}
