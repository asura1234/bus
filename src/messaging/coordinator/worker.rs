//! Coordinator worker ownership.
use super::storage::remove_spooled;
use super::{
    control_dispatch, diagnostics, io, mpsc, usage, AgentId, Arc, BTreeMap, BTreeSet, BusCommand,
    BusEvent, BusSnapshot, BusState, CoordinatorLease, Duration, JsonStore, Mutex, PathBuf,
    RequestId, RuntimeStatus, StoragePause, Transport,
};
use crate::agents::providers::spool;
use crate::messaging::perf;
use std::path::Path;

#[cfg(test)]
#[path = "tests/callback_gc_startup_test.rs"]
mod callback_gc_startup_tests;
#[cfg(test)]
#[path = "tests/callback_gc_test.rs"]
mod callback_gc_tests;
mod callback_spool;
mod callback_wake;
mod codex_settlement;
mod command_queue;
mod control_reads;
use command_queue::{collect_delivery_commands, next_command};
mod history;
mod kimi_settlement;
mod restart_notice;
mod restored_prompt;
use codex_settlement::{recorded_codex_end, saved_rollouts};
use control_reads::{interrupts_delivery, native_input_barrier};
use kimi_settlement::{recorded_kimi_end, state_provider_is_kimi};

/// How often a tick moves settled history out of `state.json`.
const ARCHIVE_EVERY: Duration = Duration::from_secs(60);

/// Private interrupt admissions; epoch ownership prevents reuse after reopening,
/// and serials are never restored by failed model saves or receipt eviction.
#[derive(Default)]
pub(super) struct InterruptAttempts {
    pub(super) epoch: Arc<()>,
    pub(super) serial: u64,
    pub(super) latest: BTreeMap<AgentId, u64>,
}

pub(super) struct Worker {
    pub(super) interrupt_attempts: InterruptAttempts,
    pub(super) idle_alerted_rooms: BTreeSet<super::RoomId>, // In-memory idle episodes.
    pub(super) disk_space: super::disk_space::DiskSpace,
    pub(super) directory_checks: super::working_directory::DirectoryChecks,
    pub(super) attention: super::attention::Runtime,
    pub(super) state: BusState,
    /// Exact last committed model; observation-only polls may advance state.
    pub(super) durable_state: BusState,
    /// Latest native-server generation observed by this coordinator.
    pub(super) serving_generation: u64,
    pub(super) store: JsonStore,
    pub(super) data_dir: PathBuf,
    pub(super) _lease: CoordinatorLease,
    pub(super) transport: Box<dyn Transport>,
    pub(super) callback_wakes: BTreeSet<AgentId>,
    pub(super) submission_rechecks: BTreeSet<AgentId>,
    pub(super) native_submissions: BTreeMap<AgentId, super::native_delivery::PendingSubmission>,
    pub(super) revision: u64,
    pub(super) last_command_id: u64,
    /// UI command ids start at 1. Control commands set this to 0.
    pub(super) active_command_id: u64,
    /// Form generation captured when the focus call now being served was accepted.
    pub(super) queued_focus_generation: Option<u64>,
    pub(super) error: Option<String>,
    pub(super) poll_failures: u32,
    pub(super) missing_binary_checked: BTreeMap<AgentId, Option<String>>,
    pub(super) storage_pause: Option<StoragePause>,
    pub(super) branch_checks: BTreeMap<AgentId, std::time::Instant>,
    /// Each agent's latest dialog wait and how many polls it has held.
    pub(super) dialog_seen: BTreeMap<AgentId, (Option<String>, u8)>,
    pub(super) delivery_waits: BTreeMap<AgentId, (RequestId, &'static str)>,
    /// Replies committed in state.json whose delivery journal did not land.
    /// Their completed_reply stays off durable_state until a later save
    /// journals the stamp. Recovery treats that one-field gap as this set.
    pub(super) unstamped_replies: BTreeSet<RequestId>,
    /// Author waiters renew these leases without acknowledging any reply.
    /// A dead waiter cannot hold routing across restart or lease expiry.
    pub(super) reply_waiters: BTreeMap<super::PromptId, std::time::Instant>,
    #[cfg(test)]
    pub(super) test_reply_waiter_now: Option<std::time::Instant>,
    #[cfg(test)]
    pub(super) test_now_ms: Option<u64>,
    pub(super) reply_delivery_retries: BTreeMap<RequestId, super::delivery::ReplyDeliveryRetry>,
    /// When each agent's provider last failed to show a pasted prompt, which
    /// the server then withdrew; the retry waits `PROMPT_NOT_SHOWN_RETRY`.
    pub(super) withdrawn_at: BTreeMap<AgentId, std::time::Instant>,
    /// When each agent last began a turn of its own that has not settled.
    pub(super) own_turns: BTreeMap<AgentId, std::time::Instant>,
    pub(super) dev_enabled: bool,
    pub(super) error_pipe: crate::utils::error_pipe::Runtime,
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
    /// The UI's settings file; set only for a real launch so tests never touch it.
    pub(super) settings_path: Option<PathBuf>,
    /// Folders listed for system sounds; None reads the operating system's.
    pub(super) sound_dirs: Option<Vec<PathBuf>>,
    /// Saves deferred to one write, while a tick applies spooled callbacks.
    pub(super) batch: Option<SaveBatch>,
    /// When the last archive pass ran; it runs at most once per [`ARCHIVE_EVERY`].
    archived_at: Option<std::time::Instant>,
    #[cfg(test)]
    pub(super) test_save_observations: Option<mpsc::Sender<super::TestSaveObservation>>,
}

/// State changes waiting for one `state.json` write, and the spool files
/// that may be deleted only once that write is durable.
#[derive(Default)]
pub(super) struct SaveBatch {
    /// The in-memory state before the first deferred change; restored when
    /// the write fails, as a failed immediate save leaves the state unchanged.
    pub(super) before: Option<(BusState, u64)>,
    acknowledgements: Vec<(PathBuf, Vec<PathBuf>)>,
    rejections: Vec<(AgentId, PathBuf, PathBuf)>,
    /// A deleted owner's captures outlive all callback acknowledgements in this batch.
    collect_deleted_callbacks: bool,
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
        state.upgrade_dialog_notice_delivery();
        state.upgrade_requester_reply_routes();
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
        let codex_rollouts = saved_rollouts(&state);
        let ids: Vec<_> = state.agents().map(|a| a.id).collect();
        for id in ids {
            super::recovery::check_saved_project_hooks(&mut state, id, &data_dir)?;
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
        super::callback_gc::collect(&data_dir, &state);
        Ok(Self {
            interrupt_attempts: InterruptAttempts::default(),
            idle_alerted_rooms: BTreeSet::new(),
            disk_space: super::disk_space::DiskSpace::default(),
            directory_checks: super::working_directory::DirectoryChecks::default(),
            attention: super::attention::Runtime::default(),
            durable_state: state.clone(),
            serving_generation: 0,
            state,
            store,
            data_dir,
            _lease: lease,
            transport,
            native_submissions: BTreeMap::new(),
            callback_wakes: BTreeSet::new(),
            submission_rechecks: BTreeSet::new(),
            revision: 0,
            last_command_id: 0,
            active_command_id: 0,
            queued_focus_generation: None,
            error: None,
            poll_failures: 0,
            missing_binary_checked: BTreeMap::new(),
            storage_pause: None,
            branch_checks: BTreeMap::new(),
            dialog_seen: BTreeMap::new(),
            delivery_waits: BTreeMap::new(),
            unstamped_replies: BTreeSet::new(),
            reply_waiters: BTreeMap::new(),
            #[cfg(test)]
            test_reply_waiter_now: None,
            #[cfg(test)]
            test_now_ms: None,
            reply_delivery_retries: BTreeMap::new(),
            withdrawn_at: BTreeMap::new(),
            own_turns: BTreeMap::new(),
            dev_enabled: false,
            error_pipe: Default::default(),
            control_receipts: BTreeMap::new(),
            control_receipt_order: std::collections::VecDeque::new(),
            control_receipt_bytes: 0,
            control_receipt_retention: control_dispatch::CONTROL_RECEIPT_RETENTION,
            usage: usage::Usage::default(),
            codex_rollouts,
            settings_path: None,
            sound_dirs: None,
            batch: None,
            archived_at: None,
            #[cfg(test)]
            test_save_observations: None,
        })
    }

    pub(super) fn now_ms(&self) -> u64 {
        #[cfg(test)]
        if let Some(now) = self.test_now_ms {
            return now;
        }
        io::now_ms()
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

    fn delay_dev_command(&self) {
        if !self.dev_enabled {
            return;
        }
        let delay = std::fs::read_to_string(self.data_dir.join("command-delay-ms"))
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(0);
        if delay > 0 {
            std::thread::sleep(Duration::from_millis(delay));
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
    pub(super) fn begin_batch(&mut self) {
        debug_assert!(self.batch.is_none(), "batches do not nest");
        self.batch = Some(SaveBatch::default());
    }

    pub(super) fn collect_deleted_callbacks(&mut self) {
        if let Some(batch) = self.batch.as_mut() {
            batch.collect_deleted_callbacks = true;
        } else {
            super::callback_gc::collect(&self.data_dir, &self.durable_state);
        }
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
        for (id, dir, path) in batch.rejections {
            spool::retain_rejected(&dir, &path, spool::RoutingKey(id.0))
                .map_err(|e| e.to_string())?;
        }
        if batch.collect_deleted_callbacks {
            super::callback_gc::collect(&self.data_dir, &self.durable_state);
        }
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
        self.queued_focus_generation = call.form_generation;
        let response = self.control_response_with_events(&call.request, Some(events));
        self.queued_focus_generation = None;
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

    fn finish_shutdown(
        &mut self,
        id: u64,
        events: &mpsc::Sender<BusEvent>,
        snapshots: &Arc<Mutex<Arc<BusSnapshot>>>,
    ) {
        self.finish_native_submissions(true);
        self.fold_journal();
        self.finish_feishu();
        self.last_command_id = id;
        if let Ok(mut shared) = snapshots.lock() {
            *shared = Arc::new(self.snapshot());
        }
        let _ = events.send(BusEvent::CommandFinished {
            command_id: id,
            result: Ok(()),
        });
    }

    pub(super) fn run(
        mut self,
        commands: mpsc::Receiver<(u64, BusCommand)>,
        events: mpsc::Sender<BusEvent>,
        snapshots: Arc<Mutex<Arc<BusSnapshot>>>,
    ) {
        crate::platform::set_current_thread_priority(crate::platform::ThreadPriority::Interactive);
        self.compact_history();
        let mut next_poll = std::time::Instant::now();
        let mut pending_commands = std::collections::VecDeque::new();
        let mut poll_overtakes = 0;
        let mut observed_native = false;
        loop {
            let timeout = if self.native_submissions.is_empty()
                && (self.callback_wakes.is_empty() || self.storage_pause.is_some())
            {
                next_poll.saturating_duration_since(std::time::Instant::now())
            } else {
                Duration::from_millis(10)
            };
            let received = next_command(&commands, &mut pending_commands, &mut poll_overtakes)
                .map(Ok)
                .unwrap_or_else(|| commands.recv_timeout(timeout));
            let busy = perf::COORDINATOR_LOOP.enter("command");
            if received
                .as_ref()
                .is_ok_and(|(_, command)| native_input_barrier(command))
            {
                busy.stage("native_input_barrier");
                self.finish_native_submissions(true);
            }
            match received {
                Ok((id, BusCommand::Shutdown)) => {
                    self.finish_shutdown(id, &events, &snapshots);
                    break;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.finish_native_submissions(true);
                    self.fold_journal();
                    self.finish_feishu();
                    break;
                }
                Ok((_, BusCommand::CallbackReady(launch))) => {
                    self.queue_callback_wake(&launch);
                    self.coalesce_callback_wakes(&mut pending_commands);
                }
                Ok((_id, BusCommand::Control(call))) => {
                    busy.stage("control");
                    if self.serve_control_without_refresh(call, &events, &snapshots, next_poll) {
                        continue;
                    }
                }
                Ok((id, command)) => {
                    self.active_command_id = id;
                    let submitting = matches!(
                        command,
                        BusCommand::Submit(..) | BusCommand::SubmitQueued(..)
                    );
                    let _span = tracing::debug_span!("bus.command", command_id = id).entered();
                    let name = perf::command_name(&command);
                    let started = std::time::Instant::now();
                    #[cfg(test)]
                    let saves_before = self.store.writes();
                    let result = if self.storage_pause.is_some() {
                        Err(self.storage_notice().into())
                    } else {
                        self.delay_dev_command();
                        self.command(command, &events)
                    };
                    perf::COMMAND.record_op(started.elapsed(), name);
                    #[cfg(test)]
                    self.observe_test_saves(Some(id), name, saves_before);
                    self.last_command_id = id;
                    let quiet_refusal = result.as_ref().err().is_some_and(|error| {
                        crate::messaging::model::is_recipient_deleted_refusal(error)
                    });
                    if submitting {
                        if quiet_refusal {
                            tracing::debug!(
                                event = "bus.message.not_sent",
                                command_id = id,
                                "Deleted recipient was not sent"
                            );
                        } else {
                            tracing::info!(
                                event = "bus.message.accepted",
                                command_id = id,
                                outcome = if result.is_ok() { "queued" } else { "rejected" },
                                "Bus submit command settled"
                            );
                        }
                    }
                    if let Err(error) = &result {
                        if !quiet_refusal {
                            self.error = Some(error.clone());
                        }
                    }
                    let _ = events.send(BusEvent::CommandFinished {
                        command_id: id,
                        result,
                    });
                    busy.stage("snapshot");
                    self.publish_snapshot(&snapshots);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            self.retry_storage(std::time::Instant::now(), &events);
            if let Err(error) = self.pump_feishu(&events, io::now_ms()) {
                self.error = Some(error);
            }
            self.finish_native_submissions(false);
            if let Err(error) = self.consume_woken_callbacks() {
                self.error = Some(error);
            }
            let can_poll = collect_delivery_commands(&commands, &mut pending_commands);
            if can_poll && observed_native && self.storage_pause.is_none() {
                if let Err(error) = self.submit_eligible_while(|| {
                    collect_delivery_commands(&commands, &mut pending_commands)
                }) {
                    self.error = Some(error);
                }
            }
            let can_poll = can_poll && collect_delivery_commands(&commands, &mut pending_commands);
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
                    observed_native = self.poll_failures == 0;
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
        self.refresh_error_pipe_at(io::now_ms());
        perf::COORDINATOR_LOOP.relabel("tick.disk_space");
        self.refresh_disk_space_at(std::time::Instant::now())?;
        if let Err(error) = self.announce_recorded_restart() {
            tracing::warn!(%error, "restart outcome was not announced");
        }
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
        self.submission_rechecks.clear();
        self.withdraw_restored_grok_prompts();
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
            if agent.deletion_pending || self.native_submissions.contains_key(&agent.id) {
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
        self.check_current_launch_directories()?;
        let mut state = self.state.clone();
        let agents: Vec<_> = state.agents().map(|a| (a.id, a.observed_at_ms)).collect();
        let mut changed = false;
        for (agent, observed_at_ms) in agents {
            // A native write still owns this recipient until its receipt.
            // Other recipients must keep reconciling on the readiness tick.
            if self.native_submissions.contains_key(&agent) {
                continue;
            }
            changed |= self.settle_previous_kimi_requests(&mut state, agent, observed_at_ms);
            let current = state.agent(agent).and_then(|owner| owner.current_request);
            let rollout = self.codex_rollouts.get(&agent);
            let kimi_recorded = recorded_kimi_end(&self.data_dir, &mut state, agent)
                .unwrap_or_else(|error| {
                    if state.record_error_episode(format!("kimi-evidence:{}:{error}", agent.0)) {
                        tracing::warn!(event = "bus.kimi.evidence_failed", agent_id = agent.0, %error, "Kimi completion evidence failed; closing the request with an actionable error");
                    }
                    Some(crate::messaging::model::RecordedTurnEnd::Error(error))
                });
            let kimi_recorded = &kimi_recorded;
            let recorded_end = |request: &crate::messaging::model::Request| {
                if state_provider_is_kimi(&self.state, request.agent_id) {
                    (Some(request.id) == current)
                        .then(|| (*kimi_recorded).clone())
                        .flatten()
                } else {
                    recorded_codex_end(rollout?, request)
                }
            };
            let mut interrupted: Vec<_> = state
                .unsettled_requests()
                .filter(|request| request.agent_id == agent)
                .map(|request| request.id)
                .chain(current)
                .collect();
            interrupted.sort_by_key(|request| (Some(*request) != current, *request));
            interrupted.dedup();
            for request in interrupted {
                changed |= state
                    .settle_generation_request(
                        agent,
                        request,
                        self.serving_generation,
                        observed_at_ms,
                        recorded_end,
                    )
                    .is_some();
            }
            changed |= state.settle_unclaimed_steering(agent, observed_at_ms, recorded_end);
            changed |= state
                .settle_ended_request(agent, observed_at_ms, recorded_end)
                .is_some();
        }
        if changed {
            self.save(state)?;
        }
        Ok(())
    }
}
