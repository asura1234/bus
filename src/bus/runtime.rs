//! The single Bus coordinator. UI commands/snapshots contain data only.
#[path = "runtime_callbacks.rs"]
mod callback_runtime;
#[path = "runtime_commands.rs"]
mod commands;
#[path = "runtime_control.rs"]
mod dev_control;
#[path = "runtime_resume.rs"]
mod resume;
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
    DeleteRoom(RoomId),
    DeleteAgent(AgentId),
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
    Dev(super::control::DevCall),
    Shutdown,
}

#[derive(Clone, Debug)]
pub(crate) enum BusEvent {
    /// Explicit dev navigation request; enqueueing does not prove it is visible yet.
    DevFocusRequested {
        room: RoomId,
        agent: Option<AgentId>,
    },
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
    _dev_control: Option<super::control::Server>,
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
    #[cfg(test)]
    pub(crate) fn test_channel(
        snapshot: Arc<BusSnapshot>,
    ) -> (Self, mpsc::Receiver<(u64, BusCommand)>) {
        let (commands, receiver) = mpsc::sync_channel(256);
        let (_, events) = mpsc::channel();
        (
            Self {
                _dev_control: None,
                commands,
                snapshots: Arc::new(Mutex::new(snapshot)),
                events,
            },
            receiver,
        )
    }

    pub(crate) fn start(data_dir: PathBuf, target: ConnectionTarget) -> Result<Self, String> {
        let mut worker = Worker::open(data_dir.clone(), Box::new(HerdrTransport::new(target)))?;
        worker.dev_enabled = super::diagnostics::dev_enabled();
        let snapshots = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
        let (commands, receiver) = mpsc::sync_channel(256);
        let dev_control = super::control::start(worker.dev_enabled, &data_dir, commands.clone())?;
        let (event_tx, events) = mpsc::channel();
        let shared = Arc::clone(&snapshots);
        std::thread::Builder::new()
            .name("bus-coordinator".into())
            .spawn(move || worker.run(receiver, event_tx, shared))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            _dev_control: dev_control,
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
    delivery_waits: BTreeMap<AgentId, (RequestId, &'static str)>,
    dev_enabled: bool,
    dev_receipts: BTreeMap<String, (super::control::Request, super::control::Response)>,
    dev_receipt_bytes: usize,
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
            delivery_waits: BTreeMap::new(),
            dev_enabled: false,
            dev_receipts: BTreeMap::new(),
            dev_receipt_bytes: 0,
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
            tracing::error!(
                event = "bus.storage.failed",
                reason = "save_failed",
                "Sending suspended; state not persisted"
            );
            return Err(format!("Bus storage failed at {}; sending is suspended. Fix storage and restart Bus: {error}",self.store.path().display()));
        }
        super::diagnostics::replies(&self.state, &state, "bus.reply.persisted");
        for agent in state.agents() {
            if self
                .state
                .agent(agent.id)
                .is_some_and(|old| old.status != agent.status)
            {
                tracing::debug!(event = "bus.agent.status", agent_id = agent.id.0,
                    room_id = agent.room_id.0, request_id = ?agent.current_request.map(|id| id.0),
                    status = ?agent.status, "Bus status changed");
            }
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
        let mut pending_command = None;
        loop {
            let timeout = next_poll.saturating_duration_since(std::time::Instant::now());
            match pending_command
                .take()
                .map(Ok)
                .unwrap_or_else(|| commands.recv_timeout(timeout))
            {
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
                Ok((_id, BusCommand::Dev(call))) => {
                    let response = self.dev_response_with_events(&call.request, Some(&events));
                    // A disconnected client does not cancel or replay a committed action.
                    let _ = call.reply.try_send(response);
                }
                Ok((id, command)) => {
                    let submitting = matches!(command, BusCommand::Submit(_));
                    let _span = tracing::debug_span!("bus.command", command_id = id).entered();
                    let result = if self.storage_failed {
                        Err("Bus storage unavailable; restart after fixing storage".into())
                    } else {
                        self.command(command, &events)
                    };
                    self.last_command_id = id;
                    if submitting {
                        tracing::info!(
                            event = "bus.message.accepted",
                            command_id = id,
                            outcome = if result.is_ok() { "queued" } else { "rejected" },
                            "Bus submit command settled"
                        );
                    }
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
            // Apply already-confirmed user commands before delivering queued
            // work. In particular, Submit followed by Delete must not send in
            // the background between those two commands.
            pending_command = commands.try_recv().ok();
            if pending_command.is_none() && std::time::Instant::now() >= next_poll {
                if !self.storage_failed {
                    if let Err(error) =
                        self.tick_with_delivery_check(|| match commands.try_recv() {
                            Ok(command) => {
                                pending_command = Some(command);
                                false
                            }
                            Err(mpsc::TryRecvError::Empty) => true,
                            Err(mpsc::TryRecvError::Disconnected) => false,
                        })
                    {
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

    #[cfg(test)]
    fn tick(&mut self) -> Result<(), String> {
        self.tick_with_delivery_check(|| true)
    }

    fn tick_with_delivery_check(
        &mut self,
        can_deliver: impl FnMut() -> bool,
    ) -> Result<(), String> {
        self.poll().inspect_err(|_| {
            tracing::warn!(
                event = "bus.coordinator.failed",
                stage = "poll",
                "Delivery paused for this tick"
            );
        })?;
        let agents: Vec<_> = self.state.agents().cloned().collect();
        for agent in agents {
            if agent.deletion_pending {
                continue;
            }
            if let Some(launch) = &agent.runtime_identity.launch_id {
                self.consume_callbacks(agent.id, &self.data_dir.join("callbacks").join(launch))
                    .inspect_err(|_| {
                        tracing::warn!(
                            event = "bus.coordinator.failed",
                            stage = "callbacks",
                            agent_id = agent.id.0,
                            "Callback processing failed; ownership retained"
                        );
                    })?;
            }
        }
        self.submit_ready_while(can_deliver).inspect_err(|_| {
            tracing::warn!(
                event = "bus.coordinator.failed",
                stage = "submit",
                "Submission could not finish"
            );
        })
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
        let mut rebound = Vec::new();
        for agent in self.state.agents() {
            let info = infos
                .iter()
                .find(|i| {
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
                })
                .or_else(|| resume::restored_agent(&self.state, agent, &infos));
            if let Some(info) = info {
                if agent.runtime_identity.terminal_id.as_deref() != Some(info.terminal_id.as_str())
                    || agent.runtime_identity.pane_id.as_deref() != Some(info.pane_id.as_str())
                {
                    let mut identity = agent.runtime_identity.clone();
                    identity.terminal_id = Some(info.terminal_id.clone());
                    identity.pane_id = Some(info.pane_id.clone());
                    state
                        .set_agent_runtime_identity(agent.id, identity)
                        .map_err(|e| e.to_string())?;
                    rebound.push((
                        agent.id,
                        agent.runtime_identity.terminal_id.clone(),
                        info.terminal_id.clone(),
                    ));
                }
            }
            let status = info.map_or(RuntimeStatus::Unavailable, |info| {
                if agent.session_binding_invalidated || agent.deletion_pending {
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
            // Hook installation is explicitly consented before the agent is
            // created. Once the native terminal reports that exact owned
            // agent as interactive, room delivery is ready too; requiring a
            // second Bus-only confirmation creates an Idle-but-undeliverable
            // deadlock (especially for Codex, whose SessionStart is deferred
            // until its first prompt).
            if info.is_some_and(|info| info.interactive_ready)
                && !agent.hook_setup_confirmed
                && !agent.session_binding_invalidated
                && !agent.deletion_pending
            {
                state
                    .confirm_hook_setup(agent.id)
                    .map_err(|e| e.to_string())?;
                state
                    .set_agent_error(agent.id, None)
                    .map_err(|e| e.to_string())?;
                tracing::info!(
                    event = "bus.agent.ready",
                    agent_id = agent.id.0,
                    provider = ?agent.provider,
                    "Owned provider terminal is interactive; room delivery enabled"
                );
            }
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
        self.save(state)?;
        for (agent, previous, current) in rebound {
            tracing::info!(event = "bus.resume.rebound", agent_id = agent.0,
                previous_terminal_id = ?previous, terminal_id = current,
                "Reconnected the saved provider conversation; request ownership preserved");
        }
        Ok(())
    }

    #[cfg(test)]
    fn submit_ready(&mut self) -> Result<(), String> {
        self.submit_ready_while(|| true)
    }

    fn submit_ready_while(&mut self, mut can_deliver: impl FnMut() -> bool) -> Result<(), String> {
        self.delivery_waits
            .retain(|id, _| self.state.agent(*id).is_some());
        let agents: Vec<_> = self.state.agents().cloned().collect();
        for agent in agents {
            // A command may have arrived while polling or sending to a prior
            // agent. Re-enter command dispatch before starting another send.
            if !can_deliver() {
                break;
            }
            if let Some(request) = self.state.queued_requests(agent.id).first().copied() {
                if let Some(reason) = super::diagnostics::wait_reason(&agent) {
                    if self.delivery_waits.insert(agent.id, (request, reason))
                        != Some((request, reason))
                    {
                        super::diagnostics::request(
                            &self.state,
                            request,
                            "bus.delivery.wait",
                            reason,
                        );
                    }
                } else {
                    self.delivery_waits.remove(&agent.id);
                }
            } else {
                self.delivery_waits.remove(&agent.id);
            }
            if agent.status != RuntimeStatus::Idle
                || !agent.hook_setup_confirmed
                || agent.session_binding_invalidated
                || agent.deletion_pending
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
            super::diagnostics::request(
                &self.state,
                request,
                "bus.delivery.start",
                "native_submit",
            );
            let started = std::time::Instant::now();
            let _span = tracing::info_span!(
                "bus.delivery",
                request_id = request.0,
                agent_id = agent.id.0,
                room_id = agent.room_id.0
            )
            .entered();
            tracing::debug!(
                event = "bus.delivery.payload",
                payload_bytes = text.len(),
                "Bus payload metadata"
            );
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
            let outcome_name = match &outcome {
                SubmissionOutcome::Confirmed { .. } => "confirmed",
                SubmissionOutcome::DefinitelyRejected { .. } => "rejected",
                SubmissionOutcome::Uncertain { .. } => "uncertain",
            };
            tracing::info!(
                event = "bus.delivery.result",
                request_id = request.0,
                outcome = outcome_name,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "Native submit result; uncertain outcomes are never retried"
            );
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
