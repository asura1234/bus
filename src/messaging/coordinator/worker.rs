//! Coordinator worker ownership.
use super::{
    dev_control, diagnostics, io, mpsc, usage, AgentId, Arc, BTreeMap, BusCommand, BusEvent,
    BusSnapshot, BusState, Duration, JsonStore, Mutex, PathBuf, RequestId, RuntimeStatus,
    Transport,
};

pub(super) struct Worker {
    pub(super) state: BusState,
    pub(super) store: JsonStore,
    pub(super) data_dir: PathBuf,
    pub(super) _lease: std::fs::File,
    pub(super) transport: Box<dyn Transport>,
    pub(super) revision: u64,
    pub(super) last_command_id: u64,
    pub(super) error: Option<String>,
    pub(super) storage_failed: bool,
    pub(super) branch_checks: BTreeMap<AgentId, std::time::Instant>,
    /// Each agent's latest dialog wait and how many polls it has held.
    pub(super) dialog_seen: BTreeMap<AgentId, (Option<String>, u8)>,
    pub(super) delivery_waits: BTreeMap<AgentId, (RequestId, &'static str)>,
    /// When each agent last began a turn of its own that has not settled.
    pub(super) own_turns: BTreeMap<AgentId, std::time::Instant>,
    pub(super) dev_enabled: bool,
    /// Mutation receipts by request ID, so a retried request replays its
    /// response instead of running twice. Oldest first in `dev_receipt_order`.
    pub(super) dev_receipts: BTreeMap<String, dev_control::DevReceipt>,
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
            dialog_seen: BTreeMap::new(),
            delivery_waits: BTreeMap::new(),
            own_turns: BTreeMap::new(),
            dev_enabled: false,
            dev_receipts: BTreeMap::new(),
            dev_receipt_order: std::collections::VecDeque::new(),
            dev_receipt_bytes: 0,
            dev_receipt_retention: dev_control::DEV_RECEIPT_RETENTION,
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
            self.storage_failed = true;
            tracing::error!(
                event = "bus.storage.failed",
                reason = "save_failed",
                "Sending suspended; state not persisted"
            );
            return Err(format!("Bus storage failed at {}; sending is suspended. Fix storage and restart Bus: {error}",self.store.path().display()));
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
        self.state = state;
        self.revision += 1;
        Ok(())
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
                    let submitting =
                        matches!(command, BusCommand::Submit(_) | BusCommand::SubmitQueued(_));
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
            let can_poll = collect_delivery_commands(&commands, &mut pending_commands);
            if can_poll && std::time::Instant::now() >= next_poll {
                if !self.storage_failed {
                    if let Err(error) = self.tick_with_delivery_check(|| {
                        collect_delivery_commands(&commands, &mut pending_commands)
                    }) {
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
        BusCommand::Dev(call) => dev_control::is_mutation(&call.request.method),
        _ => true,
    }
}
