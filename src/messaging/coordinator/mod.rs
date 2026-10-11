//! The single Bus coordinator. UI commands/snapshots contain data only.
pub(crate) mod agents;
mod delivery;
mod expiry;
mod poll;
mod storage;
mod worker;
use storage::{CoordinatorLease, StoragePause};
use worker::Worker;

#[path = "callbacks.rs"]
mod callback_runtime;
#[path = "commands.rs"]
mod commands;
#[path = "control/mod.rs"]
mod control_dispatch;
#[path = "dialogs.rs"]
mod dialogs;
pub(crate) mod resume;
#[path = "settings.rs"]
mod settings_runtime;
#[cfg(test)]
use crate::messaging::model::UNBOUND_SETTLE_MS;
use crate::messaging::{
    control::server as control,
    diagnostics,
    model::{
        AgentId, AgentRecipients, AgentRuntimeIdentity, Author, BusState, CallbackDisposition,
        CallbackEventKind, CallbackRejection, Draft, ModelError, PromptId, Provider,
        ProviderCallback, Request, RequestId, RequestPhase, Room, RoomAgent, RoomId, RoomKind,
        RuntimeStatus, SubmissionOutcome,
    },
    native::{BusTransport, Transport},
    orchestration as orchestrator,
    prefs::settings,
    storage::{io, state_store::JsonStore},
};
use crate::protocol::api::{
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
    SetRoomSound(RoomId, bool),
    /// A system sound name, or None for Bus's own ding.
    SetRoomSoundName(RoomId, Option<String>),
    /// Every work room's sound on or off, also what new work rooms start with.
    SetAllRoomsSound(bool),
    /// Every work room's sound name (None is Bus's own ding), also saved for new rooms.
    SetAllRoomsSoundName(Option<String>),
    SetMaxCompactionsPerAgent(u32),
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
        result: Result<Vec<crate::agents::providers::suggest::PathSuggestion>, String>,
    },
    AgentAdded(AgentId),
    RoomCreated(RoomId),
    TerminalFocused {
        agent: AgentId,
        pane_id: String,
    },
    StorageRecovered,
    SetupRequired {
        input: AddAgent,
        orchestrator: Option<orchestrator::OrchestratorSpec>,
        notice: crate::agents::providers::launch::SetupNotice,
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
    _control: Option<control::Server>,
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
const COMMAND_QUEUE_CAPACITY: usize = 256;

impl BusHandle {
    #[cfg(test)]
    pub(crate) fn test_channel(
        snapshot: Arc<BusSnapshot>,
    ) -> (Self, mpsc::Receiver<(u64, BusCommand)>) {
        let (commands, receiver) = mpsc::sync_channel(COMMAND_QUEUE_CAPACITY);
        let (_, events) = mpsc::channel();
        (
            Self {
                _control: None,
                commands,
                snapshots: Arc::new(Mutex::new(snapshot)),
                events,
            },
            receiver,
        )
    }

    /// A real coordinator thread over `seed`, saved to `data_dir`, whose
    /// terminal server lists no agents. The extra sender injects dev calls the
    /// way the dev control socket does.
    #[cfg(test)]
    pub(crate) fn start_for_test(
        data_dir: PathBuf,
        seed: &BusState,
    ) -> (Self, mpsc::SyncSender<(u64, BusCommand)>) {
        struct NoAgents;
        impl Transport for NoAgents {
            fn request(
                &mut self,
                _: Method,
            ) -> Result<ResponseResult, crate::messaging::native::TransportError> {
                Ok(ResponseResult::AgentList { agents: vec![] })
            }
        }
        io::private_dir(&data_dir).unwrap();
        JsonStore::new(data_dir.join("state.json"))
            .save(seed)
            .unwrap();
        let mut worker = Worker::open(data_dir, Box::new(NoAgents)).unwrap();
        worker.dev_enabled = true;
        let snapshots = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
        let (commands, receiver) = mpsc::sync_channel(COMMAND_QUEUE_CAPACITY);
        let (event_tx, events) = mpsc::channel();
        let shared = Arc::clone(&snapshots);
        std::thread::spawn(move || worker.run(receiver, event_tx, shared));
        (
            Self {
                _control: None,
                commands: commands.clone(),
                snapshots,
                events,
            },
            commands,
        )
    }

    pub(crate) fn start(data_dir: PathBuf, target: ConnectionTarget) -> Result<Self, String> {
        let mut worker = Worker::open(data_dir.clone(), Box::new(BusTransport::new(target)))?;
        worker.dev_enabled = diagnostics::dev_enabled();
        worker.settings_path = settings::path();
        if let Err(error) = worker.apply_global_settings() {
            tracing::warn!(%error, "global Bus settings not applied");
            worker.error = Some(error);
        }
        let snapshots = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
        let (commands, receiver) = mpsc::sync_channel(COMMAND_QUEUE_CAPACITY);
        // Every session runs the control socket; `dev_enabled` only gates
        // the dev-tier methods.
        let control = control::start(&data_dir, commands.clone())?;
        let (event_tx, events) = mpsc::channel();
        let shared = Arc::clone(&snapshots);
        std::thread::Builder::new()
            .name("bus-coordinator".into())
            .spawn(move || worker.run(receiver, event_tx, shared))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            _control: Some(control),
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

use crate::agents::providers::launch;
use crate::agents::providers::spool as callbacks;
pub(crate) use agents::AddAgent;
#[cfg(test)]
pub(crate) use control_dispatch::{dev_tier_mentions, Tier, METHODS};
pub(crate) mod usage;
