//! Coordinator worker ownership.
use super::storage::remove_spooled;
use super::{
    control_dispatch, diagnostics, io, mpsc, usage, AgentId, Arc, BTreeMap, BTreeSet, BusCommand,
    BusEvent, BusSnapshot, BusState, CoordinatorLease, Duration, JsonStore, Mutex, PathBuf,
    RequestId, RequestPhase, RuntimeStatus, StoragePause, Transport,
};
use crate::agents::providers::spool;
use crate::messaging::perf;
use crate::messaging::storage::state_store::JournalOp;
use std::path::Path;

mod callback_spool;
mod control_reads;
use control_reads::interrupts_delivery;

/// How often a tick moves settled history out of `state.json`.
const ARCHIVE_EVERY: Duration = Duration::from_secs(60);

pub(super) struct Worker {
    pub(super) idle_alerted_rooms: BTreeSet<super::RoomId>, // In-memory idle episodes.
    pub(super) disk_space: super::disk_space::DiskSpace,
    pub(super) directory_checks: super::working_directory::DirectoryChecks,
    pub(super) attention: super::attention::Runtime,
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
    pub(super) poll_failures: u32,
    pub(super) missing_binary_checked: BTreeMap<AgentId, Option<String>>,
    pub(super) storage_pause: Option<StoragePause>,
    pub(super) branch_checks: BTreeMap<AgentId, std::time::Instant>,
    /// Each agent's latest dialog wait and how many polls it has held.
    pub(super) dialog_seen: BTreeMap<AgentId, (Option<String>, u8)>,
    pub(super) delivery_waits: BTreeMap<AgentId, (RequestId, &'static str)>,
    /// Author waiters renew these leases without acknowledging any reply.
    /// A dead waiter cannot hold routing across restart or lease expiry.
    pub(super) reply_waiters: BTreeMap<super::PromptId, std::time::Instant>,
    #[cfg(test)]
    pub(super) test_reply_waiter_now: Option<std::time::Instant>,
    pub(super) reply_delivery_retries: BTreeMap<RequestId, super::delivery::ReplyDeliveryRetry>,
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
    /// Last observed shells and the request that observed them. A different
    /// request inherits these shells instead of waiting for their replies.
    pub(super) claude_shell_observations: BTreeMap<AgentId, (Option<RequestId>, BTreeSet<String>)>,
    /// The UI's settings file; set only for a real launch so tests never touch it.
    pub(super) settings_path: Option<PathBuf>,
    /// Folders listed for system sounds; None reads the operating system's.
    pub(super) sound_dirs: Option<Vec<PathBuf>>,
    /// Saves deferred to one write, while a tick applies spooled callbacks.
    batch: Option<SaveBatch>,
    /// When the last archive pass ran; it runs at most once per [`ARCHIVE_EVERY`].
    archived_at: Option<std::time::Instant>,
    #[cfg(test)]
    pub(super) test_save_observations: Option<mpsc::Sender<super::TestSaveObservation>>,
}

/// State changes waiting for one `state.json` write, and the spool files
/// that may be deleted only once that write is durable.
#[derive(Default)]
struct SaveBatch {
    /// The in-memory state before the first deferred change; restored when
    /// the write fails, as a failed immediate save leaves the state unchanged.
    before: Option<(BusState, u64)>,
    acknowledgements: Vec<(PathBuf, Vec<PathBuf>)>,
    rejections: Vec<(AgentId, PathBuf, PathBuf)>,
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
        state.upgrade_dialog_notice_delivery();
        state
            .upgrade_agent_names(io::now_ms())
            .map_err(|e| e.to_string())?;
        state
            .retire_queued_github_notices(io::now_ms())
            .map_err(|e| e.to_string())?;
        state
            .retire_queued_room_digests(io::now_ms())
            .map_err(|e| e.to_string())?;
        state
            .retire_queued_operational_notices(io::now_ms())
            .map_err(|e| e.to_string())?;
        // Visibility belongs to the attached client, not its persisted session.
        state.leave_room_view();
        state
            .expire_stalled_queued_requests(io::now_ms())
            .map_err(|e| e.to_string())?;
        // Recovered idle is not fresh settlement evidence; the next API poll owns it.
        let ids: Vec<_> = state.agents().map(|a| a.id).collect();
        for id in ids {
            super::recovery::check_saved_project_hooks(&mut state, id)?;
            if let Some(request) = state.agent(id).and_then(|agent| agent.current_request) {
                diagnostics::request(
                    &state,
                    request,
                    "bus.resume.open_request",
                    "loaded_before_runtime_poll",
                );
            }
            state
                .observe_status(id, RuntimeStatus::Unavailable, io::now_ms())
                .map_err(|e| e.to_string())?;
        }
        store.save(&state).map_err(|e| e.to_string())?;
        Ok(Self {
            idle_alerted_rooms: BTreeSet::new(),
            disk_space: super::disk_space::DiskSpace::default(),
            directory_checks: super::working_directory::DirectoryChecks::default(),
            attention: super::attention::Runtime::default(),
            durable_state: state.clone(),
            state,
            store,
            data_dir,
            _lease: lease,
            transport,
            revision: 0,
            last_command_id: 0,
            error: None,
            poll_failures: 0,
            missing_binary_checked: BTreeMap::new(),
            storage_pause: None,
            branch_checks: BTreeMap::new(),
            dialog_seen: BTreeMap::new(),
            delivery_waits: BTreeMap::new(),
            reply_waiters: BTreeMap::new(),
            #[cfg(test)]
            test_reply_waiter_now: None,
            reply_delivery_retries: BTreeMap::new(),
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
            claude_shell_observations: BTreeMap::new(),
            settings_path: None,
            sound_dirs: None,
            batch: None,
            archived_at: None,
            #[cfg(test)]
            test_save_observations: None,
        })
    }

    pub(super) fn snapshot(&self) -> BusSnapshot {
        BusSnapshot {
            state: self.state.clone(),
            revision: self.revision,
            last_command_id: self.last_command_id,
            error: self
                .error
                .clone()
                .or_else(|| self.attention.status.error.clone()),
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

    /// Before serving anything, deduplicates crash copies in history, so the
    /// rewrite never runs on the UI thread or between commands.
    fn compact_history(&self) {
        match self.store.compact_history() {
            Ok(true) => tracing::info!(
                event = "bus.history.compacted",
                entries = self.state.history().len(),
                "Bus history rewritten without duplicate crash copies"
            ),
            Ok(false) => {}
            Err(error) => tracing::warn!(
                event = "bus.history.compact_failed",
                %error,
                "Bus history keeps duplicate crash copies until the next start"
            ),
        }
    }

    /// Runs the archive pass at most once per [`ARCHIVE_EVERY`].
    fn archive_when_due(&mut self) -> Result<(), String> {
        let now = std::time::Instant::now();
        if self
            .archived_at
            .is_some_and(|last| now.duration_since(last) < ARCHIVE_EVERY)
        {
            return Ok(());
        }
        self.archived_at = Some(now);
        self.archive_history(io::now_ms())
    }

    /// Archives settled requests and old notices, and prunes transient routing
    /// metadata and spent ids, in one save when anything changed.
    pub(super) fn archive_history(&mut self, now_ms: u64) -> Result<(), String> {
        let mut state = self.state.clone();
        let mut changed = state.archive(now_ms);
        changed |= state.prune_attention();
        if changed {
            self.save(state)?;
        }
        Ok(())
    }

    /// Defers saves until [`Self::finish_batch`], which writes them once.
    fn begin_batch(&mut self) {
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
    fn finish_batch(&mut self) -> Result<(), String> {
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
        for (id, dir, path) in batch.rejections {
            spool::retain_rejected(&dir, &path, spool::RoutingKey(id.0))
                .map_err(|e| e.to_string())?;
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

    /// Answers one control call, unless it is a read whose caller was
    /// already answered `coordinator_timeout`: that has nothing to commit.
    fn serve_control(
        &mut self,
        call: crate::messaging::control::server::ControlCall,
        events: &mpsc::Sender<BusEvent>,
        snapshots: &Mutex<Arc<BusSnapshot>>,
    ) {
        if let Some(queued) = call.queued_for() {
            perf::CONTROL_QUEUE.record_op(queued, perf::control_method_label(&call.request.method));
            tracing::debug!(
                event = "bus.control.command.dequeued",
                control_request_id = %call.request.id.chars().take(128).collect::<String>(),
                method = perf::control_method_label(&call.request.method),
                queue_ms = queued.as_millis() as u64,
                "Control command reached the coordinator"
            );
        }
        if call
            .deadline
            .is_some_and(|deadline| std::time::Instant::now() >= deadline)
            && !control_dispatch::is_mutation(&call.request.method)
        {
            perf::CONTROL_SKIPPED.add(1);
            tracing::debug!(
                event = "bus.control.command.skipped",
                control_request_id = %call.request.id,
                method = perf::control_method_label(&call.request.method),
                "Control read skipped after its caller's deadline"
            );
            return;
        }
        let started = std::time::Instant::now();
        let response = self.control_response_with_events(&call.request, Some(events));
        perf::CONTROL.record_op(
            started.elapsed(),
            perf::control_method_label(&call.request.method),
        );
        // Publish even a failed mutation's durable pending state before its
        // reply. The following tick can block on native IO or callbacks.
        if control_dispatch::is_mutation(&call.request.method) {
            if let Ok(mut shared) = snapshots.lock() {
                *shared = self.timed_snapshot();
            }
        }
        // A disconnected client does not cancel or replay a committed action.
        let _ = call.reply.try_send(response);
    }

    pub(super) fn run(
        mut self,
        commands: mpsc::Receiver<(u64, BusCommand)>,
        events: mpsc::Sender<BusEvent>,
        snapshots: Arc<Mutex<Arc<BusSnapshot>>>,
    ) {
        // Agents' builds can saturate the machine; control replies must still run.
        crate::platform::set_current_thread_priority(crate::platform::ThreadPriority::Interactive);
        self.compact_history();
        let mut next_poll = std::time::Instant::now();
        let mut pending_commands = std::collections::VecDeque::new();
        let mut poll_overtakes = 0;
        loop {
            let timeout = next_poll.saturating_duration_since(std::time::Instant::now());
            let received = next_command(&commands, &mut pending_commands, &mut poll_overtakes)
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
                    if self.serve_control_without_refresh(call, &events, &snapshots, next_poll) {
                        continue;
                    }
                }
                Ok((id, command)) => {
                    let submitting =
                        matches!(command, BusCommand::Submit(_) | BusCommand::SubmitQueued(_));
                    let _span = tracing::debug_span!("bus.command", command_id = id).entered();
                    let name = perf::command_name(&command);
                    let started = std::time::Instant::now();
                    #[cfg(test)]
                    let saves_before = self.store.writes();
                    let result = if self.storage_pause.is_some() {
                        Err(self.storage_notice().into())
                    } else {
                        self.command(command, &events)
                    };
                    perf::COMMAND.record_op(started.elapsed(), name);
                    #[cfg(test)]
                    self.observe_test_saves(Some(id), name, saves_before);
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
                    self.publish_snapshot(&snapshots);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            self.retry_storage(std::time::Instant::now(), &events);
            if let Err(error) = self.pump_feishu(&events, io::now_ms()) {
                self.error = Some(error);
            }
            // Apply already-confirmed user commands before delivering queued
            // work. In particular, Submit followed by Delete must not send in
            // the background between those two commands.
            let can_poll = collect_delivery_commands(&commands, &mut pending_commands);
            if can_poll && std::time::Instant::now() >= next_poll {
                if self.storage_pause.is_none() {
                    busy.stage("tick");
                    let _tick = perf::TICK.start();
                    #[cfg(test)]
                    let saves_before = self.store.writes();
                    if let Err(error) = self.tick_with_delivery_check(
                        || collect_delivery_commands(&commands, &mut pending_commands),
                        &events,
                    ) {
                        if self.storage_pause.is_none() {
                            self.error = Some(error);
                        }
                    }
                    #[cfg(test)]
                    self.observe_test_saves(None, "tick", saves_before);
                }
                next_poll = std::time::Instant::now() + self.poll_interval();
            }
            busy.stage("snapshot");
            self.publish_snapshot(&snapshots);
        }
    }

    #[cfg(test)]
    fn observe_test_saves(&self, command_id: Option<u64>, operation: &'static str, before: u64) {
        if let Some(observations) = &self.test_save_observations {
            let _ = observations.send(super::TestSaveObservation {
                command_id,
                operation,
                full_saves: self.store.writes() - before,
            });
        }
    }

    fn publish_snapshot(&self, snapshots: &Mutex<Arc<BusSnapshot>>) {
        let snapshot = self.timed_snapshot();
        if let Ok(mut shared) = snapshots.lock() {
            *shared = snapshot;
        }
    }

    fn timed_snapshot(&self) -> Arc<BusSnapshot> {
        let _timer = perf::SNAPSHOT.start();
        Arc::new(self.snapshot())
    }

    #[cfg(test)]
    pub(super) fn tick(&mut self) -> Result<(), String> {
        self.tick_with_delivery_check(|| true, &mpsc::channel().0)
    }

    pub(super) fn tick_with_delivery_check(
        &mut self,
        can_deliver: impl FnMut() -> bool,
        events: &mpsc::Sender<BusEvent>,
    ) -> Result<(), String> {
        perf::COORDINATOR_LOOP.relabel("tick.disk_space");
        self.refresh_disk_space_at(std::time::Instant::now())?;
        perf::COORDINATOR_LOOP.relabel("tick.expire");
        self.expire_queued_now()?;
        perf::COORDINATOR_LOOP.relabel("tick.poll");
        let Some(infos) = self.poll().inspect_err(|_| {
            tracing::warn!(
                event = "bus.coordinator.failed",
                stage = "poll",
                "Delivery paused for this tick"
            );
        })?
        else {
            return Ok(());
        };
        // Every callback this tick applies is persisted by one write.
        perf::COORDINATOR_LOOP.relabel("tick.callbacks");
        self.begin_batch();
        let consumed = self
            .consume_all_callbacks()
            .and_then(|()| self.archive_when_due());
        perf::COORDINATOR_LOOP.relabel("tick.save_callbacks");
        let finished = self.finish_batch();
        consumed.and(finished)?;
        perf::COORDINATOR_LOOP.relabel("tick.cleanup_startups");
        self.cleanup_failed_startups(&infos, events, io::now_ms())?;
        perf::COORDINATOR_LOOP.relabel("tick.settle");
        self.settle_requests()?;
        self.expire_unconfirmed_submissions(io::now_ms())?;
        perf::COORDINATOR_LOOP.relabel("tick.notices");
        self.refresh_setting_notices()?;
        perf::COORDINATOR_LOOP.relabel("tick.submit");
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
        let mut settled_groups = Vec::new();
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
            let recorded_end =
                |request: &crate::messaging::model::Request| recorded_codex_end(rollout?, request);
            if let Some(lead) = state.settle_ended_request(agent, observed_at_ms, recorded_end) {
                changed = true;
                // Use the previous group: transcript recovery can detach an
                // unproven follower while completing the rest of the group.
                let mut members = self.state.group_members(lead);
                members.push(lead);
                members.retain(|id| {
                    self.state.request(*id).is_some_and(|request| {
                        !matches!(
                            request.phase,
                            RequestPhase::Completed | RequestPhase::Abandoned
                        )
                    })
                });
                settled_groups.push((lead, members));
            }
        }
        if changed {
            self.save(state)?;
            // Only a successful durable save is evidence of abandonment.
            for (lead, members) in settled_groups {
                let _group = tracing::info_span!("bus.settlement", lead_id = lead.0).entered();
                for id in members {
                    let reason = self.state.request(id).and_then(|request| {
                        match request.failure_reason.as_deref() {
                            Some(reason @ ("reply_not_captured" | "agent_unavailable")) => {
                                Some(reason)
                            }
                            Some(reason) if reason.starts_with("provider_error:") => {
                                Some("provider_error")
                            }
                            _ => None,
                        }
                    });
                    if let Some(reason) = reason {
                        diagnostics::request(&self.state, id, "bus.message.recovered", reason);
                    }
                }
            }
        }
        Ok(())
    }
}

/// How Codex recorded the end of `request`'s turn in its rollout: the bound
/// turn, or else the one turn that ran the typed prompt after it was sent.
/// Why nothing could be read or attributed is logged, since the request then
/// closes as `reply_not_captured`.
fn recorded_codex_end(
    rollout: &Path,
    request: &crate::messaging::model::Request,
) -> Option<crate::messaging::model::RecordedTurnEnd> {
    use crate::agents::providers::codex::usage::{
        codex_prompt_turn, codex_turn_prompts, read_codex_turn_end, CodexTurnEnd, PromptTurn,
    };
    use crate::messaging::model::RecordedTurnEnd;
    let unreadable = |error: &std::io::Error| {
        tracing::warn!(
            event = "bus.message.rollout_unreadable",
            request_id = request.id.0,
            agent_id = request.agent_id.0,
            error = %error,
            "Codex rollout could not be read for the turn's reply"
        );
    };
    let sent_ms = request.prompt.submitted_at_ms;
    let turn_id = match &request.provider_turn_id {
        Some(turn) => turn.clone(),
        None => match codex_prompt_turn(rollout, sent_ms, |prompt| {
            request.matches_callback_payload(prompt)
        }) {
            Ok(PromptTurn::Found(turn)) => turn,
            Ok(outcome) => {
                tracing::info!(
                    event = "bus.message.rollout_unattributed",
                    request_id = request.id.0,
                    agent_id = request.agent_id.0,
                    outcome = ?outcome,
                    "No rollout turn provably ran this message"
                );
                return None;
            }
            Err(error) => {
                unreadable(&error);
                return None;
            }
        },
    };
    let end = match read_codex_turn_end(rollout, &turn_id) {
        Ok(end) => end?,
        Err(error) => {
            unreadable(&error);
            return None;
        }
    };
    Some(match end {
        CodexTurnEnd::Error(message) => RecordedTurnEnd::Error(message),
        CodexTurnEnd::Final(text) => {
            let prompts = codex_turn_prompts(rollout, &turn_id, sent_ms)
                .inspect_err(unreadable)
                .unwrap_or_default();
            RecordedTurnEnd::Final {
                turn_id,
                text,
                prompts,
            }
        }
    })
}

/// How many commands may run ahead of a waiting status poll before it runs:
/// interactive calls skip the poll backlog, and polls still progress.
const STATUS_POLL_OVERTAKES: usize = 4;

/// The next command to run: the developer's oldest UI command when only
/// control reads (follower status polls) are queued ahead of it, else the
/// oldest command other than a status poll, else the oldest poll. A waiting
/// poll is overtaken at most `STATUS_POLL_OVERTAKES` times in a row, counted
/// in `overtakes`. Control mutations and shutdown are never overtaken.
fn next_command(
    commands: &mpsc::Receiver<(u64, BusCommand)>,
    pending: &mut std::collections::VecDeque<(u64, BusCommand)>,
    overtakes: &mut usize,
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
        _ => {
            // Followers poll status by the hundred; a caller's one read,
            // interrupt or delete must not wait behind them. Shutdown keeps
            // its place, so polls queued before it are still answered.
            let interactive = pending.iter().position(|(_, command)| match command {
                BusCommand::Control(call) => call.request.method != "message.status",
                _ => true,
            });
            match interactive {
                Some(0) => pending.pop_front(),
                Some(index)
                    if !matches!(pending[index].1, BusCommand::Shutdown)
                        && *overtakes < STATUS_POLL_OVERTAKES =>
                {
                    *overtakes += 1;
                    pending.remove(index)
                }
                _ => {
                    *overtakes = 0;
                    pending.pop_front()
                }
            }
        }
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
