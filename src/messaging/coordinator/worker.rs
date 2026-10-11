//! Coordinator worker ownership.
use super::{
    control_dispatch, diagnostics, io, mpsc, usage, AgentId, Arc, BTreeMap, BusCommand, BusEvent,
    BusSnapshot, BusState, CoordinatorLease, Duration, JsonStore, Mutex, PathBuf, RequestId,
    RuntimeStatus, StoragePause, Transport,
};
use crate::messaging::storage::state_store::JournalOp;

pub(super) struct Worker {
    pub(super) state: BusState,
    /// Exact last committed model; observation-only polls may advance state.
    pub(super) durable_state: BusState,
    pub(super) store: JsonStore,
    pub(super) data_dir: PathBuf,
    pub(super) _lease: CoordinatorLease,
    pub(super) transport: Box<dyn Transport>,
    pub(super) revision: u64,
    pub(super) last_command_id: u64,
    pub(super) error: Option<String>,
    pub(super) storage_pause: Option<StoragePause>,
    pub(super) branch_checks: BTreeMap<AgentId, std::time::Instant>,
    /// Each agent's latest dialog wait and how many polls it has held.
    pub(super) dialog_seen: BTreeMap<AgentId, (Option<String>, u8)>,
    pub(super) delivery_waits: BTreeMap<AgentId, (RequestId, &'static str)>,
    /// When each agent's provider last failed to show a pasted prompt, which
    /// the server then withdrew; the retry waits `PROMPT_NOT_SHOWN_RETRY`.
    pub(super) withdrawn_at: BTreeMap<AgentId, std::time::Instant>,
    /// When each agent last began a turn of its own that has not settled.
    pub(super) own_turns: BTreeMap<AgentId, std::time::Instant>,
    pub(super) dev_enabled: bool,
    /// Mutation receipts by request ID, so a retried request replays its
    /// response instead of running twice. Oldest first in `dev_receipt_order`.
    pub(super) dev_receipts: BTreeMap<String, control_dispatch::DevReceipt>,
    pub(super) dev_receipt_order: std::collections::VecDeque<String>,
    pub(super) dev_receipt_bytes: usize,
    /// How long a receipt is kept before it may be evicted to make room.
    pub(super) dev_receipt_retention: Duration,
    /// Provider allowance for dev `state`; in memory only, never persisted.
    pub(super) usage: usage::Usage,
    /// The UI's settings file; set only for a real launch so tests never touch it.
    pub(super) settings_path: Option<PathBuf>,
    /// Folders listed for system sounds; None reads the operating system's.
    pub(super) sound_dirs: Option<Vec<PathBuf>>,
}

impl Worker {
    pub(super) fn open(data_dir: PathBuf, transport: Box<dyn Transport>) -> Result<Self, String> {
        io::private_dir(&data_dir).map_err(|e| e.to_string())?;
        let lease = io::lock(&data_dir.join("coordinator.lock")).map_err(|e| {
            format!("Another Bus coordinator owns this data directory, or it is inaccessible: {e}")
        })?;
        let lease = CoordinatorLease(lease);
        let store = JsonStore::new(data_dir.join("state.json"));
        let mut state = store.load().map_err(|e| e.to_string())?.unwrap_or_default();
        // Sessions saved before MASTER existed gain it here, once, before any client sees them.
        state.ensure_master_room();
        // Visibility belongs to the attached client, not its persisted session.
        state.leave_room_view();
        state
            .expire_stalled_queued_requests(io::now_ms())
            .map_err(|e| e.to_string())?;
        // Recovered idle is not fresh settlement evidence; the next API poll owns it.
        let ids: Vec<_> = state.agents().map(|a| a.id).collect();
        for id in ids {
            state
                .observe_status(id, RuntimeStatus::Unavailable, io::now_ms())
                .map_err(|e| e.to_string())?;
        }
        store.save(&state).map_err(|e| e.to_string())?;
        Ok(Self {
            durable_state: state.clone(),
            state,
            store,
            data_dir,
            _lease: lease,
            transport,
            revision: 0,
            last_command_id: 0,
            error: None,
            storage_pause: None,
            branch_checks: BTreeMap::new(),
            dialog_seen: BTreeMap::new(),
            delivery_waits: BTreeMap::new(),
            withdrawn_at: BTreeMap::new(),
            own_turns: BTreeMap::new(),
            dev_enabled: false,
            dev_receipts: BTreeMap::new(),
            dev_receipt_order: std::collections::VecDeque::new(),
            dev_receipt_bytes: 0,
            dev_receipt_retention: control_dispatch::DEV_RECEIPT_RETENTION,
            usage: usage::Usage::default(),
            settings_path: None,
            sound_dirs: None,
        })
    }

    pub(super) fn snapshot(&self) -> BusSnapshot {
        BusSnapshot {
            state: self.state.clone(),
            revision: self.revision,
            last_command_id: self.last_command_id,
            error: self.error.clone(),
        }
    }

    pub(super) fn save(&mut self, state: BusState) -> Result<(), String> {
        if let Err(error) = self.store.save(&state) {
            self.pause_storage(&error);
            return Err(self.storage_notice().into());
        }
        diagnostics::replies(&self.state, &state, "bus.reply.persisted");
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
        self.durable_state = state.clone();
        self.state = state;
        self.revision += 1;
        Ok(())
    }

    /// Commits a composer draft edit or send through the store's journal:
    /// durable before it is acknowledged, without a full `state.json` save.
    pub(super) fn journal(&mut self, state: BusState, op: JournalOp) -> Result<(), String> {
        if let Err(error) = self.store.journal(op) {
            self.pause_storage(&error);
            return Err(self.storage_notice().into());
        }
        self.state = state;
        self.revision += 1;
        Ok(())
    }

    /// Folds journal entries into `state.json` so a binary that predates the
    /// journal, such as a cutover rollback, still sees every acknowledged edit.
    fn fold_journal(&mut self) {
        if self.storage_pause.is_none() && self.store.has_unfolded_journal() {
            let _ = self.save(self.state.clone());
        }
    }

    pub(super) fn run(
        mut self,
        commands: mpsc::Receiver<(u64, BusCommand)>,
        events: mpsc::Sender<BusEvent>,
        snapshots: Arc<Mutex<Arc<BusSnapshot>>>,
    ) {
        let interval = Duration::from_millis(500);
        let mut next_poll = std::time::Instant::now();
        let mut pending_commands = std::collections::VecDeque::new();
        loop {
            let timeout = next_poll.saturating_duration_since(std::time::Instant::now());
            match pending_commands
                .pop_front()
                .map(Ok)
                .unwrap_or_else(|| commands.recv_timeout(timeout))
            {
                Ok((id, BusCommand::Shutdown)) => {
                    self.fold_journal();
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
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.fold_journal();
                    break;
                }
                Ok((_id, BusCommand::Dev(call))) => {
                    let response = self.dev_response_with_events(&call.request, Some(&events));
                    // A disconnected client does not cancel or replay a committed action.
                    let _ = call.reply.try_send(response);
                }
                Ok((id, command)) => {
                    let submitting =
                        matches!(command, BusCommand::Submit(_) | BusCommand::SubmitQueued(_));
                    let _span = tracing::debug_span!("bus.command", command_id = id).entered();
                    let result = if self.storage_pause.is_some() {
                        Err(self.storage_notice().into())
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
                    // The client settles a command only once a snapshot shows
                    // it; do not make that wait for this loop's poll tick.
                    if let Ok(mut shared) = snapshots.lock() {
                        *shared = Arc::new(self.snapshot());
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            self.retry_storage(std::time::Instant::now(), &events);
            // Apply already-confirmed user commands before delivering queued
            // work. In particular, Submit followed by Delete must not send in
            // the background between those two commands.
            let can_poll = collect_delivery_commands(&commands, &mut pending_commands);
            if can_poll && std::time::Instant::now() >= next_poll {
                if self.storage_pause.is_none() {
                    if let Err(error) = self.tick_with_delivery_check(|| {
                        collect_delivery_commands(&commands, &mut pending_commands)
                    }) {
                        if self.storage_pause.is_none() {
                            self.error = Some(error);
                        }
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
    pub(super) fn tick(&mut self) -> Result<(), String> {
        self.tick_with_delivery_check(|| true)
    }

    pub(super) fn tick_with_delivery_check(
        &mut self,
        can_deliver: impl FnMut() -> bool,
    ) -> Result<(), String> {
        self.expire_queued_now()?;
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
        self.settle_requests()?;
        self.refresh_compaction_notices()?;
        self.submit_ready_while(can_deliver).inspect_err(|_| {
            tracing::warn!(
                event = "bus.coordinator.failed",
                stage = "submit",
                "Submission could not finish"
            );
        })
    }

    pub(super) fn settle_requests(&mut self) -> Result<(), String> {
        let mut state = self.state.clone();
        let agents: Vec<_> = state.agents().map(|a| (a.id, a.observed_at_ms)).collect();
        let mut changed = false;
        for (agent, observed_at_ms) in agents {
            changed |= state.settle_ended_request(agent, observed_at_ms).is_some();
        }
        if changed {
            self.save(state)?;
        }
        Ok(())
    }
}

/// Reads are answered after this tick, without suppressing its delivery. A
/// mutation still returns to command dispatch first, in FIFO order. Bound the
/// drain by the input channel's capacity so a read flood cannot trap this loop.
fn collect_delivery_commands(
    commands: &mpsc::Receiver<(u64, BusCommand)>,
    pending: &mut std::collections::VecDeque<(u64, BusCommand)>,
) -> bool {
    if pending
        .iter()
        .any(|(_, command)| interrupts_delivery(command))
    {
        return false;
    }
    for _ in 0..super::COMMAND_QUEUE_CAPACITY {
        match commands.try_recv() {
            Ok(command) => {
                let barrier = interrupts_delivery(&command.1);
                pending.push_back(command);
                if barrier {
                    return false;
                }
            }
            Err(mpsc::TryRecvError::Empty) => return true,
            Err(mpsc::TryRecvError::Disconnected) => return false,
        }
    }
    true
}

fn interrupts_delivery(command: &BusCommand) -> bool {
    match command {
        BusCommand::Dev(call) => control_dispatch::is_mutation(&call.request.method),
        _ => true,
    }
}
