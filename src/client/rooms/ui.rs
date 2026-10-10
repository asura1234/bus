pub(super) use super::drafts::{ComposerSize, LocalRoom};
use super::editor::Editor;
pub(super) use super::toast::Toast;
use super::{
    forms::{Form, Rename, Suggestions},
    render::View,
};
#[cfg(test)]
use crate::messaging::model::{BusState, Provider};
use crate::messaging::{
    coordinator::{BusCommand, BusEvent, BusHandle, BusSnapshot},
    model::{AgentId, RoomId, RoomKind, RuntimeStatus},
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
};

#[cfg(test)]
#[path = "tests/focus_test.rs"]
mod focus_tests;

/// Which screen the Bus view shows. A change repaints every terminal cell, so
/// cells a terminal shows differently from the last frame (which the diff
/// encoder would never revisit, like the blank right margin) heal on every
/// screen switch, even in terminals that send no focus events.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ViewKey {
    pub room: Option<RoomId>,
    pub terminal: Option<AgentId>,
    pub form: Option<std::mem::Discriminant<super::forms::Form>>,
    pub deletion: bool,
}

#[derive(Clone, Debug)]
pub(super) enum Effect {
    None,
    Text(RoomId, u64),
    Notes(RoomId, u64),
    Recipients(RoomId, u64),
    Files(RoomId),
    Submit(RoomId, u64),
    Shutdown,
}

#[derive(Clone, Debug)]
pub(super) struct Pending {
    pub id: u64,
    pub command: BusCommand,
    pub effect: Effect,
    pub enqueued: bool,
    pub result: Option<Result<(), String>>,
}

pub(in crate::client) struct BusUi {
    pub handle: Option<BusHandle>,
    pub snapshot: Arc<BusSnapshot>,
    pub room: Option<RoomId>,
    pub(super) locals: BTreeMap<RoomId, LocalRoom>,
    pub(super) pending: VecDeque<Pending>,
    pub next_id: u64,
    pub send_intent: Option<RoomId>,
    /// When the pending send was requested, for its acknowledgement log.
    pub(super) send_requested_at: Option<std::time::Instant>,
    /// The pending send waits for each agent's own turn instead of steering.
    pub(super) send_queued: bool,
    pub error: Option<String>,
    pub(super) dismissed_snapshot_error: Option<String>,
    pub(super) toast: Option<Toast>,
    pub(super) observed_notice: Option<String>,
    /// Agent notices last written to the client log, so each is logged once.
    pub(super) observed_agent_notices: Option<String>,
    pub(super) toast_animation_last_tick: Option<std::time::Instant>,
    pub(super) failed: Vec<Pending>,
    // Storage recovered after these commands were rejected but before they
    // settled; settling retries them instead of keeping them failed.
    recovered_storage_failures: BTreeSet<u64>,
    pub(super) terminal: Option<AgentId>,
    pub(super) target_pane: Option<String>,
    pub(super) native_focus_pending: bool,
    pub(super) terminal_navigation: u64,
    pub(super) form: Option<Form>,
    pub(super) deletion: Option<super::deletion::DeleteDialog>,
    pub(super) rename: Option<Rename>,
    pub(super) notes_focus: bool,
    pub(super) recipient_menu: bool,
    pub(super) recipient_index: usize,
    pub(super) suggestions: Suggestions,
    pub(super) view: View,
    pub(super) status_animation_phase: u8,
    pub(super) status_animation_last_tick: Option<std::time::Instant>,
    pub(super) main_scroll: usize,
    pub(super) history_follow_tail: bool,
    pub(super) history: super::history::History,
    pub(super) thumbnails: super::thumbnails::Thumbnails,
    /// The screen the last view showed: room, agent terminal, form, dialog.
    pub(super) view_key: Option<ViewKey>,
    /// The view switched screens since the client last repainted every cell.
    pub(super) full_repaint: bool,
    /// The client presents Kitty graphics and the host terminal draws them.
    /// The host's image protocol, or None when thumbnails are not drawn.
    pub(super) graphics: Option<super::thumbnails::Protocol>,
    /// Thumbnail bytes were composed but the client has not written them yet.
    pub(super) graphics_unconfirmed: bool,
    /// Region receiving the current left-button drag, if it started a selection.
    pub(super) drag: Option<super::selection::Region>,
    /// History selection as (anchor, head); editor selections live in `Editor`.
    pub(super) history_selection: Option<(super::selection::Point, super::selection::Point)>,
    pub(super) recipient_scroll: u16,
    pub(super) sidebar_scroll: usize,
    pub(super) file_scroll: usize,
    pub(super) last_click: Option<(super::render::Action, std::time::Instant)>,
    pub(super) detail_path: Option<String>,
    pub(super) seed_first_room: bool,
    pub(super) quitting: Option<std::time::Instant>,
    pub(super) force_exit_available: bool,
    pub exit_ready: bool,
    pub(super) history_search: Option<HistorySearch>,
    /// The open Ctrl+F search over the room's chat history.
    pub(super) chat_search: Option<super::chat_search::ChatSearch>,
    pub(super) pending_line_continue: bool,
    pub(super) last_esc: Option<std::time::Instant>,
    pub(super) settings: crate::messaging::prefs::settings::BusSettings,
    /// None keeps toggles in memory only.
    pub(super) settings_path: Option<std::path::PathBuf>,
    /// Settings focus: color blind mode, sound rows, then the compaction limit.
    pub(super) settings_field: usize,
    pub(super) settings_scroll: usize,
    /// Set only by the coordinator-owning client; `None` never plays a sound.
    pub(super) sound_config: Option<crate::utils::config::SoundConfig>,
    /// System sound names, read from the OS when Settings first opens.
    pub(super) system_sounds: Option<Vec<String>>,
    pub(super) ringer: super::ring::Ringer,
}

#[derive(Clone, Debug)]
pub(super) struct HistorySearch {
    pub query: String,
    pub selected: usize,
    pub live_draft: String,
}

impl BusUi {
    pub fn new(snapshot: Arc<BusSnapshot>) -> Self {
        // Land in the first work room; MASTER is the fallback, not the default.
        let room = snapshot
            .state
            .rooms()
            .find(|r| r.kind == RoomKind::Work)
            .or_else(|| snapshot.state.rooms().next())
            .map(|r| r.id);
        let locals = snapshot
            .state
            .rooms()
            .map(|r| (r.id, LocalRoom::from(r)))
            .collect();
        let seed_first_room = snapshot.state.is_pristine();
        Self {
            handle: None,
            snapshot,
            room,
            locals,
            pending: VecDeque::new(),
            next_id: 1,
            send_intent: None,
            send_requested_at: None,
            send_queued: false,
            recovered_storage_failures: BTreeSet::new(),
            error: None,
            dismissed_snapshot_error: None,
            toast: None,
            observed_notice: None,
            observed_agent_notices: None,
            toast_animation_last_tick: None,
            failed: Vec::new(),
            terminal: None,
            target_pane: None,
            native_focus_pending: false,
            terminal_navigation: 0,
            form: None,
            deletion: None,
            rename: None,
            notes_focus: false,
            recipient_menu: false,
            recipient_index: 0,
            suggestions: Suggestions::default(),
            view: View::default(),
            status_animation_phase: 0,
            status_animation_last_tick: None,
            main_scroll: 0,
            history_follow_tail: true,
            history: super::history::History::default(),
            thumbnails: super::thumbnails::Thumbnails::default(),
            view_key: None,
            full_repaint: false,
            graphics: None,
            graphics_unconfirmed: false,
            drag: None,
            history_selection: None,
            recipient_scroll: 0,
            sidebar_scroll: 0,
            file_scroll: 0,
            last_click: None,
            detail_path: None,
            seed_first_room,
            quitting: None,
            force_exit_available: false,
            exit_ready: false,
            history_search: None,
            chat_search: None,
            pending_line_continue: false,
            last_esc: None,
            settings: crate::messaging::prefs::settings::BusSettings::default(),
            settings_path: None,
            settings_field: 0,
            settings_scroll: 0,
            sound_config: None,
            system_sounds: None,
            ringer: super::ring::Ringer::new(std::time::Instant::now()),
        }
    }
    /// Applies immediately; a failed save keeps the choice for this run only.
    pub(super) fn toggle_color_blind_mode(&mut self) {
        let on = !self.settings.color_blind_mode;
        self.settings.color_blind_mode = on;
        if let Some(path) = &self.settings_path {
            // Change only this field; the coordinator saves the sound fields.
            match crate::messaging::prefs::settings::update(path, |settings| {
                settings.color_blind_mode = on
            }) {
                Ok(saved) => self.settings = saved,
                Err(error) => self.error = Some(error),
            }
        }
    }
    pub(super) fn queue(&mut self, command: BusCommand, effect: Effect) -> u64 {
        self.pending.retain(|p|p.enqueued || !matches!((&p.effect,&effect),
            (Effect::Text(a,_),Effect::Text(b,_)) | (Effect::Notes(a,_),Effect::Notes(b,_)) | (Effect::Recipients(a,_),Effect::Recipients(b,_)) if a==b));
        let id = self.next_id;
        self.next_id += 1;
        if let BusCommand::Submit(room) | BusCommand::SubmitQueued(room) = &command {
            tracing::info!(
                event = "bus.message.submit",
                room_id = room.0,
                command_id = id,
                "Room submit queued for coordinator"
            );
        }
        self.pending.push_back(Pending {
            id,
            command,
            effect,
            enqueued: false,
            result: None,
        });
        id
    }
    pub fn receive_event(&mut self, event: BusEvent) {
        match event {
            BusEvent::DevQuitRequested => self.request_quit(),
            BusEvent::SettingsChanged(settings) => self.settings = settings,
            BusEvent::StorageRecovered => self.receive_storage_recovered(),
            BusEvent::DevFocusRequested { room, agent } => {
                if let Some(agent) = agent {
                    // Show this agent's room in the sidebar without marking the
                    // room history read merely because its terminal was opened.
                    self.room = Some(room);
                    self.open_terminal(agent);
                } else {
                    self.open_room(room);
                }
            }
            BusEvent::CommandFinished { command_id, result } => {
                if let Some(pending) = self.pending.iter_mut().find(|p| p.id == command_id) {
                    if matches!(
                        pending.command,
                        BusCommand::Submit(_) | BusCommand::SubmitQueued(_)
                    ) {
                        // From the Enter that asked for this send to its answer.
                        let enter_to_ack_ms = self
                            .send_requested_at
                            .take()
                            .map(|at| at.elapsed().as_millis() as u64);
                        tracing::info!(
                            event = "bus.message.ack",
                            command_id,
                            outcome = if result.is_ok() { "queued" } else { "rejected" },
                            enter_to_ack_ms,
                            "Room received submit acknowledgement"
                        );
                    }
                    pending.result = Some(result);
                }
            }
            BusEvent::RoomCreated(room) => {
                self.open_room(room);
                if self.seed_first_room {
                    self.seed_first_room = false;
                    self.queue(BusCommand::SetNotes(room, "Goal: stay in the room to coordinate selected agents.\nNon-goals: automatic handoffs or permission approval.".into()), Effect::None);
                }
            }
            BusEvent::AgentAdded(agent) => {
                self.form = None;
                self.open_terminal(agent);
            }
            BusEvent::TerminalFocused { agent, pane_id }
                if self.terminal == Some(agent)
                    && self
                        .target_pane
                        .as_deref()
                        .is_none_or(|target| target == pane_id) =>
            {
                self.target_pane = Some(pane_id);
            }
            BusEvent::TerminalFocused { .. } => {
                // A newly launched agent's worker request can finish after a later
                // client-side selection. Restore that selection instead of allowing
                // the old native focus to strand its terminal behind the preview.
                self.native_focus_pending = self.terminal_pane().is_some();
            }
            BusEvent::Suggestions { query_id, result } if query_id == self.suggestions.query_id => {
                match result {
                    Ok(entries) => {
                        self.suggestions.entries = entries;
                        self.suggestions.selected = 0;
                    }
                    Err(error) => self.error = Some(error),
                }
            }
            BusEvent::SetupRequired {
                input,
                orchestrator,
                notice,
            } => {
                self.suggestions.entries.clear();
                self.suggestions.query_id += 1;
                self.form = Some(Form::Consent {
                    input,
                    orchestrator,
                    notice,
                });
            }
            _ => {}
        }
    }
    fn receive_storage_recovered(&mut self) {
        if self
            .snapshot
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("Storage paused;"))
        {
            self.dismissed_snapshot_error = self.snapshot.error.clone();
        }
        if self
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("Storage paused;"))
        {
            self.error = None;
        }
        let failed = std::mem::take(&mut self.failed);
        for pending in failed {
            if storage_paused(&pending) {
                self.retry_storage_failure(pending);
            } else {
                self.failed.push(pending);
            }
        }
        // The worker can report a rejection and recover in the same loop,
        // before its snapshot lets that rejection settle.
        let unsettled = self
            .pending
            .iter()
            .filter(|pending| storage_paused(pending))
            .map(|pending| pending.id);
        self.recovered_storage_failures.extend(unsettled);
        if let Some(agent) = self
            .terminal
            .filter(|agent| self.snapshot.state.agent(*agent).is_some())
        {
            self.queue(BusCommand::FocusTerminal(agent), Effect::None);
        }
        self.show_toast("Storage recovered");
    }

    fn retry_storage_failure(&mut self, pending: Pending) {
        match pending.effect {
            Effect::Text(room, _) => self.text_changed(room),
            Effect::Notes(room, _) => self.notes_changed(room),
            Effect::Recipients(room, _) => self.recipients_changed(room),
            effect => {
                self.queue(pending.command, effect);
            }
        }
    }

    pub fn receive_snapshot(&mut self, snapshot: Arc<BusSnapshot>) {
        crate::messaging::diagnostics::replies(
            &self.snapshot.state,
            &snapshot.state,
            "bus.reply.received",
        );
        for room in snapshot.state.rooms() {
            self.locals
                .entry(room.id)
                .or_insert_with(|| LocalRoom::from(room));
            if let Some(local) = self.locals.get_mut(&room.id) {
                // Rebuilding unchanged notes would drop their caret and selection.
                if local.notes_generation == 0 && local.notes.text != room.notes {
                    local.notes = Editor::new(room.notes.clone());
                }
            }
        }
        let previous = std::mem::replace(&mut self.snapshot, snapshot);
        if let Some(config) = &self.sound_config {
            if let Some(room) = super::ring::ringing_room(&previous.state, &self.snapshot.state)
                .filter(|_| self.ringer.allow(std::time::Instant::now()))
            {
                let name = self
                    .snapshot
                    .state
                    .room(room)
                    .and_then(|room| room.sound_name.as_deref());
                crate::platform::sound::play_named(name, config);
            }
        }
        self.reconcile_deleted_targets(&previous.state);
        self.sync_toast();
    }
    pub fn settle(&mut self) {
        while self
            .pending
            .front()
            .is_some_and(|p| p.result.is_some() && self.snapshot.last_command_id >= p.id)
        {
            let Some(pending) = self.pending.pop_front() else {
                break;
            };
            match pending.result.as_ref() {
                Some(Err(error)) => {
                    self.settle_deletion(pending.id, Some(error));
                    let recovered = self.recovered_storage_failures.remove(&pending.id);
                    let keep =
                        matches!(
                            pending.effect,
                            Effect::Text(..)
                                | Effect::Notes(..)
                                | Effect::Recipients(..)
                                | Effect::Files(..)
                        ) && super::deletion::target_exists(&pending.command, &self.snapshot.state);
                    // Only a command that is retried hides its rejection.
                    if !(recovered && keep) {
                        self.error = Some(error.clone());
                    }
                    self.send_intent = None;
                    if keep {
                        if recovered {
                            self.retry_storage_failure(pending);
                        } else {
                            self.failed.push(pending);
                        }
                    }
                }
                Some(Ok(())) => {
                    self.settle_deletion(pending.id, None);
                    match pending.effect {
                        Effect::Text(room, generation) => {
                            if let Some(local) = self.locals.get_mut(&room) {
                                local.saved_text_generation = generation;
                            }
                        }
                        Effect::Notes(room, generation) => {
                            if let Some(local) = self.locals.get_mut(&room) {
                                local.saved_notes_generation = generation;
                            }
                        }
                        Effect::Recipients(room, generation) => {
                            if let Some(local) = self.locals.get_mut(&room) {
                                local.saved_recipient_generation = generation;
                            }
                        }
                        Effect::Shutdown => self.exit_ready = true,
                        _ => {}
                    }
                    if let Effect::Submit(room, generation) = pending.effect {
                        if let Some(local) = self.locals.get_mut(&room) {
                            if local.text_generation == generation {
                                local.text = Editor::default();
                                local.composer_size = ComposerSize::Auto;
                                local.composer_scroll = None;
                            }
                        }
                    }
                    // Local editors remain authoritative; an old successful save never replaces
                    // a later generation. Only their exact command is retired here.
                }
                None => {}
            }
        }
        if !self.recovered_storage_failures.is_empty() {
            let pending = &self.pending;
            self.recovered_storage_failures
                .retain(|id| pending.iter().any(|pending| pending.id == *id));
        }
        if self.pending.is_empty() {
            if let Some(room) = self.send_intent.take() {
                let generation = self
                    .locals
                    .get(&room)
                    .map_or(0, |local| local.text_generation);
                let command = if std::mem::take(&mut self.send_queued) {
                    BusCommand::SubmitQueued(room)
                } else {
                    BusCommand::Submit(room)
                };
                self.queue(command, Effect::Submit(room, generation));
            } else if self.quitting.is_some() && self.failed.is_empty() && !self.exit_ready {
                self.queue(BusCommand::Shutdown, Effect::Shutdown);
            }
        }
    }

    /// How soon the client loop should tick again. A command waiting on the
    /// coordinator (a draft save, a send) settles only on a tick, so a 100 ms
    /// cadence would add up to that much to every step of Enter's round trip.
    pub fn tick_interval(&self) -> std::time::Duration {
        if self.pending.is_empty() && self.send_intent.is_none() {
            std::time::Duration::from_millis(100)
        } else {
            std::time::Duration::from_millis(10)
        }
    }

    /// Called only from client update handling. IO lives in the coordinator worker.
    pub fn tick(&mut self) -> bool {
        let mut changed = false;
        for _ in 0..256 {
            let Some(event) = self.handle.as_ref().and_then(BusHandle::try_event) else {
                break;
            };
            self.receive_event(event);
            changed = true;
        }
        if let Some(snapshot) = self.handle.as_ref().and_then(BusHandle::snapshot) {
            if snapshot.revision != self.snapshot.revision
                || snapshot.last_command_id != self.snapshot.last_command_id
                || snapshot.error != self.snapshot.error
            {
                self.receive_snapshot(snapshot);
                changed = true;
            }
        }
        self.settle();
        if self.quitting.is_some_and(|started| {
            started.elapsed() > std::time::Duration::from_secs(5) || !self.failed.is_empty()
        }) && !self.exit_ready
        {
            self.quitting = None;
            self.force_exit_available = true;
            self.error=Some("Unsaved changes or slow storage. Ctrl+Q retries saving; Ctrl+Shift+Q exits and may lose unsaved edits.".into());
            changed = true;
        }
        // Confirmation must reach the worker even while an earlier Submit is
        // awaiting its post-poll snapshot. Preserve command order and exact
        // acknowledgements, but enqueue the prefix through deletion together.
        let dispatch_count = self
            .deletion
            .as_ref()
            .and_then(|dialog| dialog.command_id)
            .and_then(|id| self.pending.iter().position(|pending| pending.id == id))
            .map_or(1, |index| index + 1);
        for pending in self.pending.iter_mut().take(dispatch_count) {
            if !pending.enqueued {
                if let Some(handle) = &self.handle {
                    match handle.try_send(pending.id, pending.command.clone()) {
                        Ok(()) => {
                            pending.enqueued = true;
                            changed = true;
                        }
                        Err(error) => {
                            if self.error.as_ref() != Some(&error) {
                                tracing::warn!(
                                    event = "bus.command.enqueue_failed",
                                    command_id = pending.id,
                                    "Coordinator command channel unavailable; command retained"
                                );
                                self.error = Some(error);
                                changed = true;
                            }
                            break;
                        }
                    }
                }
            }
        }
        changed |= self.tick_status_animation(std::time::Instant::now());
        changed |= self.tick_toast(std::time::Instant::now());
        changed |= self.thumbnails.stale();
        changed
    }

    fn tick_status_animation(&mut self, now: std::time::Instant) -> bool {
        const FRAME_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

        let room_animated = self.snapshot.state.rooms().any(|room| {
            matches!(
                self.snapshot.state.room_status(room.id),
                RuntimeStatus::Working | RuntimeStatus::Blocked
            )
        });
        let animated_status_visible = room_animated
            || self.room.is_some_and(|room| {
                self.snapshot.state.agents().any(|agent| {
                    agent.room_id == room
                        && agent.hook_setup_confirmed
                        && !agent.session_binding_invalidated
                        && !agent.deletion_pending
                        && matches!(
                            agent.shown_status(),
                            RuntimeStatus::Working | RuntimeStatus::Blocked
                        )
                })
            });
        if !animated_status_visible {
            self.status_animation_phase = 0;
            self.status_animation_last_tick = None;
            return false;
        }
        let Some(last_tick) = self.status_animation_last_tick else {
            self.status_animation_last_tick = Some(now);
            return false;
        };
        if now.saturating_duration_since(last_tick) < FRAME_INTERVAL {
            return false;
        }
        self.status_animation_phase = self.status_animation_phase.wrapping_add(1) % 84;
        self.status_animation_last_tick = Some(now);
        true
    }
    pub(super) fn request_quit(&mut self) {
        self.quitting = Some(std::time::Instant::now());
        let failed = std::mem::take(&mut self.failed);
        for pending in failed {
            match pending.effect {
                Effect::Text(room, _) => self.text_changed(room),
                Effect::Notes(room, _) => self.notes_changed(room),
                Effect::Recipients(room, _) => self.recipients_changed(room),
                _ => {
                    self.queue(pending.command, pending.effect);
                }
            }
        }
    }
}

fn storage_paused(pending: &Pending) -> bool {
    matches!(
        pending.result.as_ref(),
        Some(Err(error)) if error.starts_with("Storage paused;")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ui() -> (BusUi, RoomId) {
        let mut state = BusState::default();
        let room = state.create_room("bus").unwrap();
        let agent = state
            .create_agent(room, "author", Provider::Codex, "/project".into(), None)
            .unwrap();
        state.set_draft_recipients(room, [agent]).unwrap();
        (
            BusUi::new(Arc::new(BusSnapshot {
                state,
                revision: 0,
                last_command_id: 0,
                error: None,
            })),
            room,
        )
    }
    fn acknowledge(ui: &mut BusUi, id: u64, success: bool) {
        ui.receive_event(BusEvent::CommandFinished {
            command_id: id,
            result: if success {
                Ok(())
            } else {
                Err("disk full".into())
            },
        });
        let mut snapshot = (*ui.snapshot).clone();
        snapshot.last_command_id = id;
        snapshot.revision += 1;
        ui.receive_snapshot(Arc::new(snapshot));
        ui.settle();
    }

    #[test]
    fn storage_recovery_retries_local_edits_and_refreshes_selected_terminal() {
        let (mut ui, room) = ui();
        let agent = ui.snapshot.state.agents().next().unwrap().id;
        ui.open_terminal(agent);
        ui.pending.clear();
        ui.locals
            .get_mut(&room)
            .unwrap()
            .text
            .insert("retained draft");
        ui.failed.push(Pending {
            id: 1,
            command: BusCommand::SetDraftText(room, "retained draft".into()),
            effect: Effect::Text(room, 1),
            enqueued: true,
            result: Some(Err("Storage paused; retrying".into())),
        });
        ui.error = Some("Storage paused; retrying".into());

        ui.receive_event(BusEvent::StorageRecovered);

        assert!(ui.failed.is_empty());
        assert!(ui.error.is_none());
        assert!(ui.pending.iter().any(|pending| {
            matches!(&pending.command, BusCommand::SetDraftText(id, _) if *id == room)
        }));
        assert!(ui.pending.iter().any(|pending| {
            matches!(&pending.command, BusCommand::FocusTerminal(id) if *id == agent)
        }));
        assert_eq!(ui.toast.as_ref().unwrap().message, "Storage recovered");
    }
    #[test]
    fn delivery_logs_receipt_once_when_reply_snapshot_reaches_room() {
        let (mut ui, room) = ui();
        let agent = ui.snapshot.state.agents().next().unwrap().id;
        let mut snapshot = (*ui.snapshot).clone();
        snapshot
            .state
            .set_draft_text(room, "PRIVATE_PROMPT")
            .unwrap();
        let request = snapshot.state.submit_draft(room, 1).unwrap()[0];
        // Project a completed reply from the coordinator, as the UI receives it.
        let mut value = serde_json::to_value(&snapshot.state).unwrap();
        value["rooms"][room.0.to_string()]["latest_replies"][agent.0.to_string()] = serde_json::json!({"request_id": request.0, "agent_id": agent.0,
                "text": "PRIVATE_REPLY", "received_at_ms": 2});
        snapshot.state = serde_json::from_value(value).unwrap();
        snapshot.revision += 1;
        let capture = crate::utils::logging::test_capture::Capture::default();
        capture.run(|| {
            ui.receive_snapshot(Arc::new(snapshot.clone()));
            ui.receive_snapshot(Arc::new(snapshot));
        });
        let logs = capture.text();
        assert_eq!(logs.matches("bus.reply.received").count(), 1, "{logs}");
        assert!(
            logs.contains(&format!("request_id={}", request.0)),
            "{logs}"
        );
        assert!(!logs.contains("PRIVATE_"), "{logs}");
        assert_eq!(
            ui.snapshot.state.room(room).unwrap().latest_replies[&agent].text,
            "PRIVATE_REPLY"
        );
    }
    #[test]
    fn immediate_enter_waits_for_own_success_and_snapshot_before_submit() {
        let (mut ui, room) = ui();
        ui.locals.get_mut(&room).unwrap().text.insert("latest");
        ui.text_changed(room);
        ui.request_send(room);
        ui.receive_event(BusEvent::CommandFinished {
            command_id: 1,
            result: Ok(()),
        });
        ui.settle();
        assert!(!ui
            .pending
            .iter()
            .any(|p| matches!(p.command, BusCommand::Submit(_))));
        acknowledge(&mut ui, 1, true);
        assert!(
            matches!(ui.pending.front().map(|p| &p.command), Some(BusCommand::Submit(r)) if *r == room)
        );
    }
    #[test]
    fn failed_save_cancels_send_and_later_success_never_acknowledges_it() {
        let (mut ui, room) = ui();
        ui.locals
            .get_mut(&room)
            .unwrap()
            .text
            .insert("newer than saved");
        ui.text_changed(room);
        ui.queue(BusCommand::LeaveRoom, Effect::None);
        ui.request_send(room);
        acknowledge(&mut ui, 1, false);
        acknowledge(&mut ui, 2, true);
        assert!(ui.send_intent.is_none());
        assert!(!ui
            .pending
            .iter()
            .any(|p| matches!(p.command, BusCommand::Submit(_))));
        assert_eq!(ui.locals[&room].text.text, "newer than saved");
        assert!(ui.error.as_deref().is_some_and(|e| e.contains("disk full")));
    }
    #[test]
    fn old_submit_ack_preserves_newer_text_and_another_room_draft() {
        let (mut ui, room) = ui();
        ui.locals.get_mut(&room).unwrap().text.insert("sent prompt");
        ui.text_changed(room);
        let saved = ui.pending.front().expect("draft save").id;
        ui.request_send(room);
        acknowledge(&mut ui, saved, true);
        let id = ui.pending.front().expect("submit").id;
        ui.locals.get_mut(&room).unwrap().text.insert("next prompt");
        ui.text_changed(room);
        acknowledge(&mut ui, id, true);
        assert_eq!(ui.locals[&room].text.text, "sent promptnext prompt");
    }
}
