//! The single Bus coordinator. UI commands/snapshots contain data only.
#[path = "runtime_callbacks.rs"]
mod callback_runtime;
#[path = "runtime_commands.rs"]
mod commands;
use super::{
    callbacks::{self, Parsed},
    launch::{self, AddAgent},
    model::*,
    store::JsonStore,
    transport::{HerdrTransport, Transport},
};
use crate::api::{
    client::ConnectionTarget,
    schema::{self, Method, ResponseResult},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};

#[derive(Clone, Debug)]
pub(crate) enum BusCommand {
    CreateRoom(String),
    RenameRoom(RoomId, String),
    RenameAgent(AgentId, String),
    SelectRoom(RoomId),
    LeaveRoom,
    #[allow(dead_code)] // Optional worker API; the shell marks seen with SelectRoom.
    MarkRoomSeen(RoomId),
    SetNotes(RoomId, String),
    SetDraftText(RoomId, String),
    SetRecipients(RoomId, BTreeSet<AgentId>),
    #[allow(dead_code)]
    // Worker API for persisted drafts; shell quotes into its newer local editor.
    Quote(RoomId, AgentId),
    AttachFile(RoomId, String),
    RemoveFile(RoomId, PathBuf),
    Submit(RoomId),
    SetDetails(AgentId, bool),
    AddAgent(AddAgent),
    FocusTerminal(AgentId),
    CompleteHookSetup(AgentId),
    Suggestions {
        query_id: u64,
        input: String,
        directories_only: bool,
    },
    Shutdown,
}

#[derive(Clone, Debug)]
pub(crate) enum BusEvent {
    /// Outcome of this exact command, independent of later snapshot acknowledgements.
    CommandFinished {
        command_id: u64,
        result: Result<(), String>,
    },
    Suggestions {
        query_id: u64,
        result: Result<Vec<launch::PathSuggestion>, String>,
    },
    AgentAdded(AgentId),
    RoomCreated(RoomId),
    TerminalFocused {
        agent: AgentId,
        pane_id: String,
    },
    SetupRequired {
        input: AddAgent,
        notice: launch::SetupNotice,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct BusSnapshot {
    pub(crate) state: BusState,
    pub(crate) revision: u64,
    pub(crate) last_command_id: u64,
    pub(crate) error: Option<String>,
}

pub(crate) struct BusHandle {
    commands: mpsc::SyncSender<(u64, BusCommand)>,
    snapshots: Arc<Mutex<Arc<BusSnapshot>>>,
    events: mpsc::Receiver<BusEvent>,
}

pub(crate) fn default_data_dir() -> Result<PathBuf, String> {
    Ok(std::env::home_dir()
        .ok_or("Home directory unavailable")?
        .join(".local/share/bus"))
}

pub(crate) const DEFAULT_SESSION: &str = "bus";

impl BusHandle {
    pub(crate) fn start(data_dir: PathBuf, target: ConnectionTarget) -> Result<Self, String> {
        let worker = Worker::open(data_dir, Box::new(HerdrTransport::new(target)))?;
        let snapshots = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
        let (commands, receiver) = mpsc::sync_channel(256);
        let (event_tx, events) = mpsc::channel();
        let shared = Arc::clone(&snapshots);
        std::thread::Builder::new()
            .name("bus-coordinator".into())
            .spawn(move || worker.run(receiver, event_tx, shared))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            commands,
            snapshots,
            events,
        })
    }

    pub(crate) fn try_send(&self, command_id: u64, command: BusCommand) -> Result<(), String> {
        self.commands
            .try_send((command_id, command))
            .map_err(|e| e.to_string())
    }

    /// Read once in event/update handling; never block compose/render.
    pub(crate) fn snapshot(&self) -> Option<Arc<BusSnapshot>> {
        self.snapshots
            .try_lock()
            .ok()
            .map(|value| Arc::clone(&value))
    }
    pub(crate) fn try_event(&self) -> Option<BusEvent> {
        self.events.try_recv().ok()
    }
}

struct Worker {
    state: BusState,
    store: JsonStore,
    data_dir: PathBuf,
    _lease: std::fs::File,
    transport: Box<dyn Transport>,
    revision: u64,
    last_command_id: u64,
    error: Option<String>,
    storage_failed: bool,
    branch_checks: BTreeMap<AgentId, std::time::Instant>,
}

impl Worker {
    fn open(data_dir: PathBuf, transport: Box<dyn Transport>) -> Result<Self, String> {
        super::io::private_dir(&data_dir).map_err(|e| e.to_string())?;
        let lease = super::io::lock(&data_dir.join("coordinator.lock")).map_err(|e| {
            format!("Another Bus coordinator owns this data directory, or it is inaccessible: {e}")
        })?;
        let store = JsonStore::new(data_dir.join("state.json"));
        let mut state = store.load().map_err(|e| e.to_string())?.unwrap_or_default();
        // Visibility belongs to the attached client, not its persisted session.
        state.leave_room_view();
        // Recovered idle is not fresh settlement evidence; the next API poll owns it.
        let ids: Vec<_> = state.agents().map(|a| a.id).collect();
        for id in ids {
            state
                .observe_status(id, RuntimeStatus::Unavailable, super::io::now_ms())
                .map_err(|e| e.to_string())?;
        }
        store.save(&state).map_err(|e| e.to_string())?;
        Ok(Self {
            state,
            store,
            data_dir,
            _lease: lease,
            transport,
            revision: 0,
            last_command_id: 0,
            error: None,
            storage_failed: false,
            branch_checks: BTreeMap::new(),
        })
    }

    fn snapshot(&self) -> BusSnapshot {
        BusSnapshot {
            state: self.state.clone(),
            revision: self.revision,
            last_command_id: self.last_command_id,
            error: self.error.clone(),
        }
    }

    fn save(&mut self, state: BusState) -> Result<(), String> {
        if let Err(error) = self.store.save(&state) {
            self.storage_failed = true;
            return Err(format!("Bus storage failed at {}; sending is suspended. Fix storage and restart Bus: {error}",self.store.path().display()));
        }
        self.state = state;
        self.revision += 1;
        Ok(())
    }

    fn run(
        mut self,
        commands: mpsc::Receiver<(u64, BusCommand)>,
        events: mpsc::Sender<BusEvent>,
        snapshots: Arc<Mutex<Arc<BusSnapshot>>>,
    ) {
        let interval = Duration::from_millis(500);
        let mut next_poll = std::time::Instant::now();
        loop {
            let timeout = next_poll.saturating_duration_since(std::time::Instant::now());
            match commands.recv_timeout(timeout) {
                Ok((id, BusCommand::Shutdown)) => {
                    self.last_command_id = id;
                    if let Ok(mut shared) = snapshots.lock() {
                        *shared = Arc::new(self.snapshot());
                    }
                    let _ = events.send(BusEvent::CommandFinished {
                        command_id: id,
                        result: Ok(()),
                    });
                    break;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Ok((id, command)) => {
                    let result = if self.storage_failed {
                        Err("Bus storage unavailable; restart after fixing storage".into())
                    } else {
                        self.command(command, &events)
                    };
                    self.last_command_id = id;
                    if let Err(error) = &result {
                        self.error = Some(error.clone());
                    }
                    let _ = events.send(BusEvent::CommandFinished {
                        command_id: id,
                        result,
                    });
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if std::time::Instant::now() >= next_poll {
                if !self.storage_failed {
                    if let Err(error) = self.tick() {
                        self.error = Some(error);
                    }
                }
                next_poll = std::time::Instant::now() + interval;
            }
            let snapshot = Arc::new(self.snapshot());
            if let Ok(mut shared) = snapshots.lock() {
                *shared = snapshot;
            }
        }
    }

    fn tick(&mut self) -> Result<(), String> {
        self.poll()?;
        let agents: Vec<_> = self.state.agents().cloned().collect();
        for agent in agents {
            if let Some(launch) = &agent.runtime_identity.launch_id {
                self.consume_callbacks(agent.id, &self.data_dir.join("callbacks").join(launch))?;
            }
        }
        self.submit_ready()
    }

    fn poll(&mut self) -> Result<(), String> {
        let result = self
            .transport
            .request(Method::AgentList(schema::EmptyParams {}));
        let infos = match result {
            Ok(ResponseResult::AgentList { agents }) => agents,
            other => {
                let mut state = self.state.clone();
                for id in self.state.agents().map(|a| a.id) {
                    state
                        .observe_status(id, RuntimeStatus::Unavailable, super::io::now_ms())
                        .map_err(|e| e.to_string())?;
                }
                self.save(state)?;
                return Err(format!(
                    "Herdr unavailable; queues and active requests retained: {other:?}"
                ));
            }
        };
        let mut state = self.state.clone();
        for agent in self.state.agents() {
            let info = infos.iter().find(|i| {
                Some(i.terminal_id.as_str()) == agent.runtime_identity.terminal_id.as_deref()
                    && Some(i.pane_id.as_str()) == agent.runtime_identity.pane_id.as_deref()
                    && i.agent.as_deref() == Some(launch::provider_kind(agent.provider))
                    && agent
                        .runtime_identity
                        .session_id
                        .as_ref()
                        .is_none_or(|expected| {
                            i.agent_session
                                .as_ref()
                                .is_some_and(|session| &session.value == expected)
                        })
            });
            let status = info.map_or(RuntimeStatus::Unavailable, |info| {
                if agent.session_binding_invalidated {
                    return RuntimeStatus::Unavailable;
                }
                if info.launch_pending {
                    return RuntimeStatus::Launching;
                }
                if !info.interactive_ready {
                    return RuntimeStatus::Unavailable;
                }
                match info.agent_status {
                    schema::AgentStatus::Idle | schema::AgentStatus::Done => RuntimeStatus::Idle,
                    schema::AgentStatus::Working => RuntimeStatus::Working,
                    schema::AgentStatus::Blocked => RuntimeStatus::Blocked,
                    schema::AgentStatus::Unknown => RuntimeStatus::Unavailable,
                }
            });
            state
                .observe_status(agent.id, status, super::io::now_ms())
                .map_err(|e| e.to_string())?;
            if let Some(info) = info {
                let cwd = info
                    .foreground_cwd
                    .as_ref()
                    .or(info.cwd.as_ref())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| agent.cwd.clone());
                // Branch lookup only when visible metadata changes; bounded one process per agent per poll is avoided.
                let branch = if cwd != agent.cwd
                    || self
                        .branch_checks
                        .get(&agent.id)
                        .is_none_or(|time| time.elapsed() >= Duration::from_secs(10))
                {
                    self.branch_checks
                        .insert(agent.id, std::time::Instant::now());
                    branch_for(&cwd)
                } else {
                    agent.branch.clone()
                };
                state
                    .update_agent_runtime_metadata(agent.id, cwd, branch)
                    .map_err(|e| e.to_string())?;
            }
        }
        self.save(state)
    }

    fn submit_ready(&mut self) -> Result<(), String> {
        let agents: Vec<_> = self.state.agents().cloned().collect();
        for agent in agents {
            if agent.status != RuntimeStatus::Idle
                || !agent.hook_setup_confirmed
                || agent.session_binding_invalidated
            {
                continue;
            }
            let identity = &agent.runtime_identity;
            let (Some(launch), Some(terminal), Some(pane)) = (
                &identity.launch_id,
                &identity.terminal_id,
                &identity.pane_id,
            ) else {
                continue;
            };
            if identity.session_id.is_none() && agent.provider != Provider::Codex {
                continue;
            }
            let Some(request) = self.state.next_queued_request(agent.id) else {
                continue;
            };
            let text = self
                .state
                .request(request)
                .ok_or("Missing queued request")?
                .prompt
                .rendered_payload();
            let boundary = callbacks::boundary(&self.data_dir.join("callbacks").join(launch))
                .map_err(|e| e.to_string())?;
            let mut state = self.state.clone();
            state
                .begin_submission(request, launch, boundary)
                .map_err(|e| e.to_string())?;
            self.save(state)?;
            let method = if let Some(session) = &identity.session_id {
                Method::AgentPromptIfIdle(schema::AgentPromptIfIdleParams {
                    target: pane.clone(),
                    text,
                    expected_terminal_id: terminal.clone(),
                    expected_pane_id: pane.clone(),
                    expected_agent: launch::provider_kind(agent.provider).into(),
                    expected_session_id: session.clone(),
                })
            } else {
                Method::AgentPromptIfUnbound(schema::AgentPromptIfUnboundParams {
                    target: pane.clone(),
                    text,
                    expected_terminal_id: terminal.clone(),
                    expected_pane_id: pane.clone(),
                    expected_managed_name: format!("bus-r{}-a{}", agent.room_id.0, agent.id.0),
                })
            };
            let outcome = match self.transport.request(method) {
                Ok(ResponseResult::AgentPrompted { .. }) => SubmissionOutcome::Confirmed {
                    provider_session_id: identity.session_id.clone(),
                    provider_turn_id: None,
                },
                Ok(other) => SubmissionOutcome::Uncertain {
                    message: format!("Unexpected submit response; no automatic retry: {other:?}"),
                },
                Err(error) if error.definitely_rejected => SubmissionOutcome::DefinitelyRejected {
                    message: error.message,
                },
                Err(error) => SubmissionOutcome::Uncertain {
                    message: format!(
                        "Submit outcome unknown; no automatic retry: {}",
                        error.message
                    ),
                },
            };
            let mut state = self.state.clone();
            state
                .record_submission(request, outcome)
                .map_err(|e| e.to_string())?;
            self.save(state)?;
        }
        Ok(())
    }
}

fn branch_for(cwd: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["-C"])
        .arg(cwd)
        .args(["branch", "--show-current"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!branch.is_empty()).then_some(branch)
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
