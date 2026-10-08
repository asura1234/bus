//! The single Bus coordinator. UI commands/snapshots contain data only.
mod agents;
mod delivery;
mod poll;
mod worker;
use worker::Worker;

#[path = "callbacks.rs"]
mod callback_runtime;
#[path = "commands.rs"]
mod commands;
#[path = "control/mod.rs"]
mod dev_control;
#[path = "dialogs.rs"]
mod dialogs;
pub(crate) mod resume;
#[path = "settings.rs"]
mod settings_runtime;
use crate::api::{
    client::ConnectionTarget,
    schema::{self, Method, ResponseResult},
};
#[cfg(test)]
use crate::messaging::model::UNBOUND_SETTLE_MS;
use crate::messaging::{
    control::server as control,
    diagnostics,
    model::{
        Agent, AgentId, AgentRecipients, AgentRuntimeIdentity, Author, BusState,
        CallbackDisposition, CallbackEventKind, CallbackRejection, Draft, ModelError, PromptId,
        Provider, ProviderCallback, Request, RequestId, RequestPhase, Room, RoomId, RoomKind,
        RuntimeStatus, SubmissionOutcome,
    },
    native::{HerdrTransport, Transport},
    orchestration as orchestrator,
    prefs::settings,
    storage::{io, state_store::JsonStore},
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
    SetRoomSound(RoomId, bool),
    /// A system sound name, or None for Bus's own ding.
    SetRoomSoundName(RoomId, Option<String>),
    /// Every work room's sound on or off, also what new work rooms start with.
    SetAllRoomsSound(bool),
    /// Every work room's sound name (None is Bus's own ding), also saved for new rooms.
    SetAllRoomsSoundName(Option<String>),
    RenameAgent(AgentId, String),
    DeleteRoom(RoomId),
    DeleteAgent(AgentId),
    SelectRoom(RoomId),
    LeaveRoom,
    /// Dev `room seen`; the shell marks rooms seen with SelectRoom.
    MarkRoomSeen(RoomId),
    SetNotes(RoomId, String),
    SetDraftText(RoomId, String),
    SetRecipients(RoomId, AgentRecipients),
    AttachFile(RoomId, String),
    RemoveFile(RoomId, PathBuf),
    Submit(RoomId),
    /// Like `Submit`, but each message waits for its agent's own turn.
    SubmitQueued(RoomId),
    SetDetails(AgentId, bool),
    AddAgent(AddAgent),
    /// Adds a MASTER agent, launched with its orchestrator system prompt.
    AddOrchestrator(AddAgent, orchestrator::OrchestratorSpec),
    FocusTerminal(AgentId),
    CompleteHookSetup(AgentId),
    Suggestions {
        query_id: u64,
        input: String,
        directories_only: bool,
    },
    Dev(control::DevCall),
    Shutdown,
}

#[derive(Clone, Debug)]
pub(crate) enum BusEvent {
    /// Explicit dev navigation request; enqueueing does not prove it is visible yet.
    DevFocusRequested {
        room: RoomId,
        agent: Option<AgentId>,
    },
    /// Dev `quit`: run the UI's own save-and-quit, as Ctrl+Q does.
    DevQuitRequested,
    /// The coordinator saved these global settings; the UI applies them.
    SettingsChanged(settings::BusSettings),
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
        orchestrator: Option<orchestrator::OrchestratorSpec>,
        notice: launch::SetupNotice,
    },
    /// A deletion finished, but these terminals were no longer Bus-owned and were left open.
    TerminalsLeftOpen(Vec<LeftOpenTerminal>),
}

/// The native terminal a deleted agent was last bound to, left untouched.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) struct LeftOpenTerminal {
    pub(crate) agent_id: AgentId,
    pub(crate) agent_name: String,
    pub(crate) pane_id: String,
    pub(crate) terminal_id: String,
}

#[derive(Clone, Debug)]
pub(crate) struct BusSnapshot {
    pub(crate) state: BusState,
    pub(crate) revision: u64,
    pub(crate) last_command_id: u64,
    pub(crate) error: Option<String>,
}

pub(crate) struct BusHandle {
    _dev_control: Option<control::Server>,
    commands: mpsc::SyncSender<(u64, BusCommand)>,
    snapshots: Arc<Mutex<Arc<BusSnapshot>>>,
    events: mpsc::Receiver<BusEvent>,
}

pub(crate) const DEFAULT_SESSION: &str = "bus";
/// Why a MASTER agent needs a room: its system prompt names that room at launch.
pub(crate) const MASTER_AGENT_NEEDS_ROOM: &str = "A MASTER agent orchestrates exactly one work room for its whole life: add it with --orchestrates ROOM (create the work room first)";

/// How long Bus holds delivery after an agent begins a turn of its own,
/// unless that turn's Stop arrives first.
const OWN_TURN_GRACE: Duration = Duration::from_secs(10);

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
        worker.dev_enabled = diagnostics::dev_enabled();
        worker.settings_path = settings::path();
        if let Err(error) = worker.apply_global_settings() {
            tracing::warn!(%error, "global Bus settings not applied");
            worker.error = Some(error);
        }
        let snapshots = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
        let (commands, receiver) = mpsc::sync_channel(256);
        let dev_control = control::start(worker.dev_enabled, &data_dir, commands.clone())?;
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

#[cfg(test)]
#[path = "tests/steering_test.rs"]
mod steering_tests;
#[cfg(test)]
#[path = "tests/poll_test.rs"]
mod tests;

use crate::agents::providers::spool as callbacks;
use crate::messaging::launch;
pub(crate) use agents::AddAgent;
pub(crate) mod usage;
