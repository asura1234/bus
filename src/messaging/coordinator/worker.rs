//! Coordinator worker ownership.
use super::{
    control_dispatch, diagnostics, io, mpsc, usage, AgentId, Arc, BTreeMap, BTreeSet, BusCommand,
    BusEvent, BusSnapshot, BusState, CoordinatorLease, Duration, JsonStore, Mutex, PathBuf,
    RequestId, RuntimeStatus, StoragePause, Transport,
};
use crate::messaging::perf;
use crate::messaging::storage::state_store::JournalOp;
use std::path::Path;

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
    /// response instead of running twice. Oldest first in `control_receipt_order`.
    pub(super) control_receipts: BTreeMap<String, control_dispatch::ControlReceipt>,
    pub(super) control_receipt_order: std::collections::VecDeque<String>,
    pub(super) control_receipt_bytes: usize,
    /// How long a receipt is kept before it may be evicted to make room.
    pub(super) control_receipt_retention: Duration,
    /// Provider allowance for dev `state`; in memory only, never persisted.
    pub(super) usage: usage::Usage,
    /// Each Codex agent's rollout, from its hooks. A turn that a provider
    /// error ended fires no Stop hook; only the rollout records why.
    pub(super) codex_rollouts: BTreeMap<AgentId, PathBuf>,
    /// The background shells each Claude agent had running when its last
    /// reply settled. Shells beyond these were started by the current
    /// request's turns, and its reply waits for them. In memory only: after a
    /// restart every running shell counts as older.
    pub(super) claude_settled_shells: BTreeMap<AgentId, BTreeSet<String>>,
    /// The UI's settings file; set only for a real launch so tests never touch it.
    pub(super) settings_path: Option<PathBuf>,
    /// Folders listed for system sounds; None reads the operating system's.
    pub(super) sound_dirs: Option<Vec<PathBuf>>,
    /// Saves deferred to one write, while a tick applies spooled callbacks.
    pub(super) batch: Option<SaveBatch>,
}

/// State changes waiting for one `state.json` write, and the spool files
/// that may be deleted only once that write is durable.
#[derive(Default)]
pub(super) struct SaveBatch {
    /// The in-memory state before the first deferred change; restored when
    /// the write fails, as a failed immediate save leaves the state unchanged.
    before: Option<(BusState, u64)>,
    acknowledgements: Vec<(PathBuf, Vec<PathBuf>)>,
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
            control_receipts: BTreeMap::new(),
            control_receipt_order: std::collections::VecDeque::new(),
            control_receipt_bytes: 0,
            control_receipt_retention: control_dispatch::CONTROL_RECEIPT_RETENTION,
            usage: usage::Usage::default(),
            codex_rollouts: BTreeMap::new(),
            claude_settled_shells: BTreeMap::new(),
            settings_path: None,
            sound_dirs: None,
            batch: None,
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

    /// Commits `state` and makes it durable. A state that differs from the
    /// last write only in status-only observations writes nothing: those are
    /// never persisted on their own (see `apply_poll`), so rewriting the same
    /// durable content is pure cost. Inside a batch the write waits for
    /// [`Self::finish_batch`].
    pub(super) fn save(&mut self, state: BusState) -> Result<(), String> {
        if !self.store.has_unfolded_journal()
            && !state.durable_poll_change_from(&self.durable_state)
        {
            if state != self.state {
                self.state = state;
                self.revision += 1;
            }
            return Ok(());
        }
        if let Some(batch) = self.batch.as_mut() {
            if batch.before.is_none() {
                batch.before = Some((self.state.clone(), self.revision));
            }
            self.state = state;
            self.revision += 1;
            return Ok(());
        }
        self.write(state)?;
        self.revision += 1;
        Ok(())
    }

    fn write(&mut self, state: BusState) -> Result<(), String> {
        if let Err(error) = self.store.save(&state) {
            self.pause_storage(&error);
            return Err(self.storage_notice().into());
        }
        diagnostics::replies(&self.durable_state, &state, "bus.reply.persisted");
        for agent in state.agents() {
            if self
                .durable_state
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
        Ok(())
    }

    /// Defers saves until [`Self::finish_batch`], which writes them once.
    pub(super) fn begin_batch(&mut self) {
        debug_assert!(self.batch.is_none(), "batches do not nest");
        self.batch = Some(SaveBatch::default());
    }

    /// Deletes consumed spool files now, or after the batch's write.
    pub(super) fn acknowledge_spool(
        &mut self,
        dir: &Path,
        paths: Vec<PathBuf>,
    ) -> Result<(), String> {
        if let Some(batch) = self.batch.as_mut() {
            batch.acknowledgements.push((dir.to_path_buf(), paths));
            return Ok(());
        }
        remove_spooled(dir, &paths)
    }

    /// Writes the batch's changes once, then deletes the spool files they
    /// consumed. When the write fails, the state returns to where the batch
    /// began and every spool file stays, so nothing is acknowledged unsaved.
    pub(super) fn finish_batch(&mut self) -> Result<(), String> {
        let Some(batch) = self.batch.take() else {
            return Ok(());
        };
        if let Some((before, revision)) = batch.before {
            let state = std::mem::take(&mut self.state);
            if let Err(error) = self.write(state) {
                self.state = before;
                self.revision = revision + 1;
                return Err(error);
            }
        }
        for (dir, paths) in batch.acknowledgements {
            remove_spooled(&dir, &paths)?;
        }
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
            let received = next_command(&commands, &mut pending_commands)
                .map(Ok)
                .unwrap_or_else(|| commands.recv_timeout(timeout));
            let busy = perf::COORDINATOR_LOOP.enter("command");
            match received {
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
                Ok((_id, BusCommand::Control(call))) => {
                    busy.stage("control");
                    let started = std::time::Instant::now();
                    let response = self.control_response_with_events(&call.request, Some(&events));
                    perf::CONTROL.record_op(
                        started.elapsed(),
                        perf::control_method_label(&call.request.method),
                    );
                    // A disconnected client does not cancel or replay a committed action.
                    let _ = call.reply.try_send(response);
                }
                Ok((id, command)) => {
                    let submitting =
                        matches!(command, BusCommand::Submit(_) | BusCommand::SubmitQueued(_));
                    let _span = tracing::debug_span!("bus.command", command_id = id).entered();
                    let name = perf::command_name(&command);
                    let started = std::time::Instant::now();
                    let result = if self.storage_pause.is_some() {
                        Err(self.storage_notice().into())
                    } else {
                        self.command(command, &events)
                    };
                    perf::COMMAND.record_op(started.elapsed(), name);
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
                    busy.stage("snapshot");
                    let snapshot = self.timed_snapshot();
                    if let Ok(mut shared) = snapshots.lock() {
                        *shared = snapshot;
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
                    busy.stage("tick");
                    let _tick = perf::TICK.start();
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
            busy.stage("snapshot");
            let snapshot = self.timed_snapshot();
            if let Ok(mut shared) = snapshots.lock() {
                *shared = snapshot;
            }
        }
    }

    fn timed_snapshot(&self) -> Arc<BusSnapshot> {
        let _timer = perf::SNAPSHOT.start();
        Arc::new(self.snapshot())
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
        // Every callback this tick applies is persisted by one write.
        self.begin_batch();
        let consumed = self.consume_all_callbacks();
        let finished = self.finish_batch();
        consumed.and(finished)?;
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

    fn consume_all_callbacks(&mut self) -> Result<(), String> {
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
        Ok(())
    }

    pub(super) fn settle_requests(&mut self) -> Result<(), String> {
        let mut state = self.state.clone();
        let agents: Vec<_> = state.agents().map(|a| (a.id, a.observed_at_ms)).collect();
        let mut changed = false;
        let hold_cap_ms = u64::from(
            self.settings_path
                .as_deref()
                .and_then(|path| crate::messaging::prefs::settings::load(path).ok())
                .unwrap_or_default()
                .background_shell_hold_minutes,
        ) * 60_000;
        for (agent, observed_at_ms) in agents {
            changed |= state
                .release_shell_hold(agent, observed_at_ms, hold_cap_ms)
                .is_some();
            let rollout = self.codex_rollouts.get(&agent);
            let provider_error = |request: &crate::messaging::model::Request| {
                let turn = request.provider_turn_id.as_deref()?;
                crate::agents::providers::codex::usage::read_codex_turn_error(rollout?, turn)
                    .ok()
                    .flatten()
            };
            changed |= state
                .settle_ended_request(agent, observed_at_ms, provider_error)
                .is_some();
        }
        if changed {
            self.save(state)?;
        }
        Ok(())
    }
}

/// The next command to run: the developer's oldest UI command when only
/// control reads (follower status polls) are queued ahead of it, else the
/// oldest command. Control mutations and shutdown are never overtaken.
fn next_command(
    commands: &mpsc::Receiver<(u64, BusCommand)>,
    pending: &mut std::collections::VecDeque<(u64, BusCommand)>,
) -> Option<(u64, BusCommand)> {
    for _ in 0..super::COMMAND_QUEUE_CAPACITY {
        match commands.try_recv() {
            Ok(command) => pending.push_back(command),
            Err(_) => break,
        }
    }
    let ui = pending.iter().position(|(_, command)| match command {
        BusCommand::Control(call) => control_dispatch::is_mutation(&call.request.method),
        _ => true,
    });
    match ui {
        Some(index)
            if !matches!(
                pending[index].1,
                BusCommand::Control(_) | BusCommand::Shutdown
            ) =>
        {
            pending.remove(index)
        }
        _ => pending.pop_front(),
    }
}

fn remove_spooled(dir: &Path, paths: &[PathBuf]) -> Result<(), String> {
    for path in paths {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    crate::platform::sync_parent_directory(dir).map_err(|e| e.to_string())
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
        BusCommand::Control(call) => control_dispatch::is_mutation(&call.request.method),
        _ => true,
    }
}
