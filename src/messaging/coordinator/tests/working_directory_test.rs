use super::*;

fn directory_fixture(
    provider: Provider,
    status: schema::AgentStatus,
) -> (Worker, AgentId, PathBuf, PathBuf) {
    let native = match provider {
        Provider::ClaudeCode => "claude",
        Provider::Codex => "codex",
        Provider::Cursor => "cursor",
    };
    let mut info = native_info("pane", native, "session");
    info.interactive_ready = true;
    info.agent_status = status;
    let (mut worker, agent, _, dir, _) = fixture(provider, Vec::new());
    worker.transport = Box::new(DirectoryTransport(info));
    let cwd = dir.join("worktree");
    std::fs::create_dir(&cwd).unwrap();
    let mut state = worker.state.clone();
    state
        .update_agent_runtime_metadata(agent, cwd.clone(), None)
        .unwrap();
    worker.save(state).unwrap();
    (worker, agent, dir, cwd)
}

struct DirectoryTransport(schema::AgentInfo);

impl Transport for DirectoryTransport {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::AgentList(_) => Ok(ResponseResult::AgentList {
                agents: vec![self.0.clone()],
            }),
            _ => Ok(ResponseResult::Ok {}),
        }
    }
}

fn poll_until(worker: &mut Worker, agent: AgentId, predicate: impl Fn(&RoomAgent) -> bool) {
    let started = std::time::Instant::now();
    loop {
        worker.poll().unwrap();
        if predicate(worker.state.agent(agent).unwrap()) {
            return;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "directory observation never reached expected state"
        );
        std::thread::yield_now();
    }
}

#[test]
fn deleted_working_directory_surfaces_recovery_action_for_every_provider() {
    for (provider, status, expected) in [Provider::Codex, Provider::ClaudeCode, Provider::Cursor]
        .into_iter()
        .flat_map(|provider| {
            [
                (provider, schema::AgentStatus::Idle, RuntimeStatus::Idle),
                (
                    provider,
                    schema::AgentStatus::Working,
                    RuntimeStatus::Working,
                ),
            ]
        })
    {
        let (mut worker, agent, dir, cwd) = directory_fixture(provider, status);
        worker.poll().unwrap();
        assert!(worker
            .state
            .agent(agent)
            .unwrap()
            .actionable_error
            .is_none());
        std::fs::remove_dir(&cwd).unwrap();
        poll_until(&mut worker, agent, |a| a.actionable_error.is_some());
        let observed = worker.state.agent(agent).unwrap();
        let error = observed
            .actionable_error
            .as_deref()
            .expect("missing cwd must be visible on the agent");
        assert!(error.contains(cwd.to_str().unwrap()), "{error}");
        assert!(
            error.contains("Restore") && error.contains("--pwd"),
            "{error}"
        );
        assert!(
            observed.details_disclosed,
            "the warning must not hide behind collapsed details"
        );
        assert_eq!(
            observed.status, expected,
            "do not stop or restart the provider"
        );
        assert!(observed.working_directory_error.is_some());
        assert_eq!(
            JsonStore::new(dir.join("state.json"))
                .load()
                .unwrap()
                .unwrap()
                .agent(agent)
                .unwrap()
                .actionable_error
                .as_deref(),
            Some(error)
        );
        std::fs::create_dir(&cwd).unwrap();
        poll_until(&mut worker, agent, |a| a.working_directory_error.is_none());
        assert!(worker
            .state
            .agent(agent)
            .unwrap()
            .actionable_error
            .is_none());
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn directory_recovery_keeps_an_unrelated_agent_error() {
    let (mut worker, agent, dir, cwd) =
        directory_fixture(Provider::Codex, schema::AgentStatus::Idle);
    std::fs::remove_dir(&cwd).unwrap();
    poll_until(&mut worker, agent, |a| a.working_directory_error.is_some());
    worker
        .state
        .set_agent_error(agent, Some("permission denied by provider".into()))
        .unwrap();
    worker.poll().unwrap();
    assert_eq!(
        worker
            .state
            .agent(agent)
            .unwrap()
            .actionable_error
            .as_deref(),
        Some("permission denied by provider")
    );
    std::fs::create_dir(&cwd).unwrap();
    poll_until(&mut worker, agent, |a| a.working_directory_error.is_none());
    assert_eq!(
        worker
            .state
            .agent(agent)
            .unwrap()
            .actionable_error
            .as_deref(),
        Some("permission denied by provider")
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_file_replacing_the_working_directory_also_shows_the_warning() {
    let (mut worker, agent, dir, cwd) =
        directory_fixture(Provider::Codex, schema::AgentStatus::Idle);
    std::fs::remove_dir(&cwd).unwrap();
    std::fs::write(&cwd, b"not a directory").unwrap();
    poll_until(&mut worker, agent, |a| a.actionable_error.is_some());
    assert!(worker
        .state
        .agent(agent)
        .unwrap()
        .actionable_error
        .is_some());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_later_callback_cannot_permanently_erase_a_missing_directory_warning() {
    let (mut worker, agent, dir, cwd) =
        directory_fixture(Provider::Codex, schema::AgentStatus::Idle);
    std::fs::remove_dir(&cwd).unwrap();
    poll_until(&mut worker, agent, |a| a.working_directory_error.is_some());
    let warning = worker.state.agent(agent).unwrap().actionable_error.clone();
    assert!(warning.is_some());
    // Turn starts/completions legitimately clear the transient agent error.
    worker.state.set_agent_error(agent, None).unwrap();
    poll_until(&mut worker, agent, |a| a.actionable_error.is_some());
    assert_eq!(worker.state.agent(agent).unwrap().actionable_error, warning);
    std::fs::create_dir(&cwd).unwrap();
    poll_until(&mut worker, agent, |a| a.working_directory_error.is_none());
    assert!(worker
        .state
        .agent(agent)
        .unwrap()
        .working_directory_error
        .is_none());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

struct MissingPane;

impl Transport for MissingPane {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::AgentList(_) => Ok(ResponseResult::AgentList { agents: Vec::new() }),
            Method::PaneGet(_) => Err(TransportError {
                code: Some("pane_not_found".into()),
                message: "pane not found".into(),
                definitely_rejected: false,
            }),
            _ => Ok(ResponseResult::Ok {}),
        }
    }
}

#[test]
fn missing_directory_does_not_hide_detached_pane_recovery() {
    let (mut worker, agent, dir, cwd) =
        directory_fixture(Provider::Codex, schema::AgentStatus::Idle);
    std::fs::remove_dir(&cwd).unwrap();
    poll_until(&mut worker, agent, |a| a.working_directory_error.is_some());
    assert!(worker
        .state
        .agent(agent)
        .unwrap()
        .working_directory_error
        .is_some());
    worker.transport = Box::new(MissingPane);
    worker.poll().unwrap();
    assert!(worker
        .state
        .agent(agent)
        .unwrap()
        .runtime_identity
        .pane_id
        .is_none());
    let detached_error = worker.state.agent(agent).unwrap().actionable_error.clone();
    std::fs::create_dir(&cwd).unwrap();
    poll_until(&mut worker, agent, |a| a.working_directory_error.is_none());
    let restored_error = worker.state.agent(agent).unwrap().actionable_error.clone();
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        detached_error
            .as_deref()
            .is_some_and(|error| error.contains("bus agent resume")),
        "missing pane recovery while directory absent: {detached_error:?}"
    );
    assert!(
        restored_error
            .as_deref()
            .is_some_and(|error| error.contains("bus agent resume")),
        "missing pane recovery after directory restoration: {restored_error:?}"
    );
}
