//! Private client drafts and read positions. These never travel to the room owner.
use super::{BusUi, Effect, Pending};
use crate::messaging::{
    coordinator::BusCommand,
    model::{Draft, RoomId},
    storage::io,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    room: Option<RoomId>,
    #[serde(default)]
    drafts: BTreeMap<RoomId, Draft>,
    #[serde(default)]
    seen: BTreeMap<RoomId, u64>,
    /// Message identities at the watermark distinguish a batch with one timestamp.
    #[serde(default)]
    seen_ids: BTreeMap<RoomId, BTreeSet<u64>>,
}

pub(super) struct ClientState {
    path: PathBuf,
    pub(super) files: BTreeMap<RoomId, Vec<PathBuf>>,
    pub(super) saved_drafts: BTreeMap<RoomId, Draft>,
    pub(super) restored_room: Option<RoomId>,
    seen: BTreeMap<RoomId, u64>,
    seen_ids: BTreeMap<RoomId, BTreeSet<u64>>,
}

fn key(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn identity() -> String {
    let explicit = std::env::var("BUS_CLIENT_ID")
        .ok()
        .filter(|value| !value.is_empty());
    let session = ["TERM_SESSION_ID", "WT_SESSION"]
        .into_iter()
        .find_map(|name| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.is_empty())
                .map(|value| format!("{name}:{value}"))
        });
    #[cfg(unix)]
    let tty = {
        // ttyname_r writes into our bounded buffer and does not retain its address.
        let mut buffer = [0i8; 1024];
        if unsafe { libc::ttyname_r(libc::STDIN_FILENO, buffer.as_mut_ptr(), buffer.len()) } == 0 {
            Some(
                unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) }
                    .to_string_lossy()
                    .into_owned(),
            )
        } else {
            None
        }
    };
    #[cfg(not(unix))]
    let tty: Option<String> = None;
    identity_parts(
        explicit.as_deref(),
        session.as_deref(),
        tty.as_deref(),
        std::process::id(),
    )
}

fn identity_parts(
    explicit: Option<&str>,
    session: Option<&str>,
    tty: Option<&str>,
    pid: u32,
) -> String {
    if let Some(explicit) = explicit {
        return format!("BUS_CLIENT_ID:{explicit}");
    }
    // tmux panes inherit the same terminal session variable. Their TTYs remain
    // distinct, so neither pane can overwrite the other's drafts or read marks.
    match (session, tty) {
        (Some(session), Some(tty)) => format!("{session}:tty:{tty}"),
        (None, Some(tty)) => format!("tty:{tty}"),
        (Some(session), None) => session.to_owned(),
        (None, None) => format!("process:{pid}"),
    }
}

impl BusUi {
    pub(in crate::client) fn saved_machine(root: &Path) -> Option<String> {
        let path = root
            .join("clients")
            .join(key(&identity()))
            .join("active-machine.json");
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    }

    pub(in crate::client) fn save_active_machine(root: &Path, machine: &str) -> Result<(), String> {
        let path = root
            .join("clients")
            .join(key(&identity()))
            .join("active-machine.json");
        io::private_dir(path.parent().expect("client state parent")).map_err(|e| e.to_string())?;
        let bytes = serde_json::to_vec(machine).map_err(|e| e.to_string())?;
        io::atomic_write(&path, &bytes).map_err(|e| e.to_string())
    }

    pub(in crate::client) fn set_client_focused(&mut self, focused: bool) {
        self.outer_focused = Some(focused);
    }

    pub(in crate::client) fn enable_client_state(
        &mut self,
        root: &Path,
        machine: &str,
    ) -> Result<(), String> {
        self.enable_client_state_for(root, machine, &identity())
    }

    fn enable_client_state_for(
        &mut self,
        root: &Path,
        machine: &str,
        identity: &str,
    ) -> Result<(), String> {
        let path = root
            .join("clients")
            .join(key(identity))
            .join(format!("{}.json", key(machine)));
        io::private_dir(path.parent().expect("client state parent")).map_err(|e| e.to_string())?;
        let saved = match std::fs::read(&path) {
            Ok(bytes) => Some(
                serde_json::from_slice::<Saved>(&bytes)
                    .map_err(|e| format!("Invalid client state: {e}"))?,
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.to_string()),
        };
        let mut files = BTreeMap::new();
        // Never seed one client's composer with another client's durable room draft.
        for (id, local) in &mut self.locals {
            let draft = saved
                .as_ref()
                .and_then(|s| s.drafts.get(id))
                .cloned()
                .unwrap_or_default();
            local.text = super::editor::Editor::new(draft.text);
            local.recipients = draft.recipient_ids;
            files.insert(*id, draft.files);
        }
        let seen = saved.as_ref().map(|s| s.seen.clone()).unwrap_or_else(|| {
            self.snapshot
                .state
                .rooms()
                .map(|room| (room.id, self.room_activity(room.id)))
                .collect()
        });
        let seen_ids = saved
            .as_ref()
            .map(|s| s.seen_ids.clone())
            .unwrap_or_else(|| {
                self.snapshot
                    .state
                    .rooms()
                    .map(|room| {
                        (
                            room.id,
                            self.room_activity_ids(room.id, self.room_activity(room.id)),
                        )
                    })
                    .collect()
            });
        if let Some(saved) = &saved {
            self.room = saved
                .room
                .filter(|id| self.snapshot.state.room(*id).is_some())
                .or(self.room);
        }
        let saved_drafts = saved.as_ref().map(|s| s.drafts.clone()).unwrap_or_default();
        let restored_room = saved
            .as_ref()
            .and_then(|s| s.room)
            .filter(|room| self.snapshot.state.room(*room).is_none());
        for (room, draft) in &saved_drafts {
            files.entry(*room).or_insert_with(|| draft.files.clone());
        }
        self.client_state = Some(ClientState {
            path,
            files,
            seen,
            seen_ids,
            saved_drafts,
            restored_room,
        });
        Ok(())
    }

    fn room_activity(&self, room: RoomId) -> u64 {
        let replies = self
            .snapshot
            .state
            .room(room)
            .into_iter()
            .flat_map(|room| room.latest_replies.values())
            .map(|r| r.received_at_ms);
        let notices = self.snapshot.state.notices(room).map(|p| p.submitted_at_ms);
        replies.chain(notices).max().unwrap_or(0)
    }

    fn room_activity_ids(&self, room: RoomId, at: u64) -> BTreeSet<u64> {
        let replies = self
            .snapshot
            .state
            .room(room)
            .into_iter()
            .flat_map(|room| room.latest_replies.values())
            .filter(|reply| reply.received_at_ms == at)
            .map(|reply| reply.request_id.0);
        let notices = self
            .snapshot
            .state
            .notices(room)
            .filter(|prompt| prompt.submitted_at_ms == at)
            .map(|prompt| prompt.id.0);
        replies.chain(notices).collect()
    }

    pub(super) fn room_unread(&self, room: RoomId) -> bool {
        match &self.client_state {
            Some(local) => {
                let at = self.room_activity(room);
                let seen = local.seen.get(&room).copied().unwrap_or(0);
                at > seen
                    || at == seen
                        && self.room_activity_ids(room, at).iter().any(|id| {
                            !local
                                .seen_ids
                                .get(&room)
                                .is_some_and(|ids| ids.contains(id))
                        })
            }
            None => self
                .snapshot
                .state
                .room(room)
                .is_some_and(|r| r.unseen_to_developer),
        }
    }

    pub(super) fn update_client_seen(&mut self) {
        let Some(room) = self.room.filter(|_| {
            self.terminal.is_none() && self.form.is_none() && self.outer_focused != Some(false)
        }) else {
            return;
        };
        if self.mark_client_seen(room) {
            if let Err(error) = self.save_client_state() {
                tracing::warn!(event = "bus.client_state.save_failed", %error);
            }
        }
    }

    fn mark_client_seen(&mut self, room: RoomId) -> bool {
        let mark = self.room_activity(room);
        let ids = self.room_activity_ids(room, mark);
        let Some(local) = self.client_state.as_mut() else {
            return false;
        };
        let seen = local.seen.get(&room).copied().unwrap_or(0);
        if mark < seen {
            return false;
        }
        if mark > seen {
            local.seen.insert(room, mark);
            local.seen_ids.insert(room, ids);
            return true;
        }
        let saved = local.seen_ids.entry(room).or_default();
        let previous = saved.len();
        saved.extend(ids);
        saved.len() != previous
    }

    pub(in crate::client) fn save_client_state(&self) -> Result<(), String> {
        let Some(local) = &self.client_state else {
            return Ok(());
        };
        let mut drafts = local.saved_drafts.clone();
        drafts.extend(self.locals.iter().map(|(room, draft)| {
            (
                *room,
                Draft {
                    text: draft.text.text.clone(),
                    recipient_ids: draft.recipients.clone(),
                    files: local.files.get(room).cloned().unwrap_or_default(),
                },
            )
        }));
        let saved = Saved {
            room: local.restored_room.or(self.room),
            drafts,
            seen: local.seen.clone(),
            seen_ids: local.seen_ids.clone(),
        };
        let bytes = serde_json::to_vec(&saved).map_err(|e| e.to_string())?;
        io::atomic_write(&local.path, &bytes).map_err(|e| e.to_string())
    }

    pub(super) fn restore_client_rooms(&mut self, state: &crate::messaging::model::BusState) {
        for room in state.rooms() {
            self.locals.entry(room.id).or_insert_with(|| {
                let mut local = super::drafts::LocalRoom::from(room);
                if let Some(client) = &self.client_state {
                    let draft = client
                        .saved_drafts
                        .get(&room.id)
                        .cloned()
                        .unwrap_or_default();
                    local.text = super::editor::Editor::new(draft.text);
                    local.recipients = draft.recipient_ids;
                }
                local
            });
        }
        if let Some(local) = self.client_state.as_mut() {
            if let Some(room) = local
                .restored_room
                .filter(|room| state.room(*room).is_some())
            {
                self.room = Some(room);
                local.restored_room = None;
            }
        }
    }

    /// True means the command is entirely local and must never reach the coordinator.
    pub(super) fn handle_client_command(
        &mut self,
        id: u64,
        command: &BusCommand,
        effect: &Effect,
    ) -> bool {
        if self.client_state.is_none() {
            return false;
        }
        let local_command = match command {
            BusCommand::SelectRoom(room) | BusCommand::MarkRoomSeen(room) => {
                self.mark_client_seen(*room);
                if let Some(local) = self.client_state.as_mut() {
                    local.restored_room = None;
                }
                true
            }
            BusCommand::LeaveRoom
            | BusCommand::SetDraftText(..)
            | BusCommand::SetRecipients(..) => true,
            BusCommand::AttachFile(room, raw) => {
                let path = if self.remote_machine {
                    PathBuf::from(raw)
                } else {
                    super::drafts::attachment_path(raw)
                };
                let Some(local) = self.client_state.as_mut() else {
                    return false;
                };
                let files = local.files.entry(*room).or_default();
                if !files.contains(&path) {
                    files.push(path);
                }
                true
            }
            BusCommand::RemoveFile(room, path) => {
                let Some(local) = self.client_state.as_mut() else {
                    return false;
                };
                local.files.entry(*room).or_default().retain(|p| p != path);
                true
            }
            BusCommand::Shutdown if !self.owns_coordinator => {
                if let Err(error) = self.save_client_state() {
                    self.error = Some(error);
                } else {
                    self.exit_ready = true;
                }
                return true;
            }
            _ => false,
        };
        if local_command {
            let result = self.save_client_state();
            if result.is_ok() {
                match effect {
                    Effect::Text(room, generation) => {
                        if let Some(local) = self.locals.get_mut(room) {
                            local.saved_text_generation = *generation;
                        }
                    }
                    Effect::Recipients(room, generation) => {
                        if let Some(local) = self.locals.get_mut(room) {
                            local.saved_recipient_generation = *generation;
                        }
                    }
                    _ => {}
                }
            } else {
                self.locally_completed.insert(id);
                self.pending.push_back(Pending {
                    id,
                    command: command.clone(),
                    effect: effect.clone(),
                    enqueued: true,
                    result: Some(result),
                });
            }
            return true;
        }
        if !self.machine_available {
            self.locally_completed.insert(id);
            self.pending.push_back(Pending {
                id,
                command: command.clone(),
                effect: effect.clone(),
                enqueued: true,
                result: Some(Err(format!(
                    "{} is Unavailable; command was not sent",
                    self.machine_label
                ))),
            });
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::{
        coordinator::BusSnapshot,
        model::{BusState, Provider},
    };
    use std::sync::Arc;

    fn fixture() -> (BusUi, RoomId) {
        let mut state = BusState::default();
        let room = state.create_room("work").unwrap();
        let agent = state
            .create_agent(room, "author", Provider::Codex, "/project".into(), None)
            .unwrap();
        state.set_draft_recipients(room, [agent]).unwrap();
        state.set_draft_text(room, "foreign draft").unwrap();
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

    #[test]
    fn multi_machine_client_drafts_selection_and_seen_never_enter_coordinator_queue() {
        let root = crate::utils::test_temp::unique_temp_path("multi-machine-client");
        let (mut a, room) = fixture();
        let (mut b, _) = fixture();
        a.enable_client_state_for(&root, "remote", "client-a")
            .unwrap();
        b.enable_client_state_for(&root, "remote", "client-b")
            .unwrap();
        assert_eq!(a.locals[&room].text.text, "");
        a.locals.get_mut(&room).unwrap().text.insert("private A");
        a.text_changed(room);
        a.open_room(room);
        assert!(a.pending.is_empty());
        assert_eq!(b.locals[&room].text.text, "");
        assert_eq!(
            a.snapshot.state.room(room).unwrap().draft.text,
            "foreign draft"
        );
        let (mut restored, _) = fixture();
        restored
            .enable_client_state_for(&root, "remote", "client-a")
            .unwrap();
        assert_eq!(restored.locals[&room].text.text, "private A");
        assert_eq!(restored.room, Some(room));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn multi_machine_remote_attachment_is_literal_and_offline_mutation_is_failed_once() {
        let root = crate::utils::test_temp::unique_temp_path("multi-machine-path");
        let (mut ui, room) = fixture();
        ui.enable_client_state_for(&root, "remote", "client")
            .unwrap();
        ui.set_machine("remote".into(), true);
        ui.attach_paths(room, vec!["~/only-on-remote.png".into()]);
        assert_eq!(
            ui.current_draft_files(room),
            [PathBuf::from("~/only-on-remote.png")]
        );
        ui.set_machine_available(false);
        ui.queue(BusCommand::CreateRoom("unsent".into()), Effect::None);
        ui.settle();
        assert!(ui.pending.is_empty());
        assert!(ui.failed.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn multi_machine_reading_one_client_does_not_clear_another_client_unread() {
        let root = crate::utils::test_temp::unique_temp_path("multi-machine-unread");
        let (mut a, room) = fixture();
        let (mut b, _) = fixture();
        a.enable_client_state_for(&root, "remote", "reader-a")
            .unwrap();
        b.enable_client_state_for(&root, "remote", "reader-b")
            .unwrap();
        a.set_client_focused(false);
        b.set_client_focused(false);
        let mut next = (*a.snapshot).clone();
        let author = next.state.agents().next().unwrap().id;
        next.state
            .post_to_developer(room, author, "new report".into(), vec![], 42)
            .unwrap();
        next.revision += 1;
        a.receive_snapshot(Arc::new(next.clone()));
        b.receive_snapshot(Arc::new(next.clone()));
        assert!(a.room_unread(room));
        assert!(b.room_unread(room));
        a.open_room(room);
        assert!(!a.room_unread(room));
        assert!(b.room_unread(room));
        next.state.mark_room_seen(room).unwrap();
        next.revision += 1;
        b.receive_snapshot(Arc::new(next));
        assert!(
            b.room_unread(room),
            "the remote Bus read flag cannot clear this client"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn multi_machine_empty_initial_snapshot_does_not_erase_saved_drafts() {
        let root = crate::utils::test_temp::unique_temp_path("multi-machine-late-snapshot");
        let (mut a, room) = fixture();
        a.enable_client_state_for(&root, "remote", "reader")
            .unwrap();
        a.locals
            .get_mut(&room)
            .unwrap()
            .text
            .insert("survives reconnect");
        a.text_changed(room);
        let full = Arc::clone(&a.snapshot);
        let mut b = BusUi::new(Arc::new(BusSnapshot {
            state: BusState::default(),
            revision: 0,
            last_command_id: 0,
            error: None,
        }));
        b.enable_client_state_for(&root, "remote", "reader")
            .unwrap();
        b.save_client_state().unwrap();
        b.receive_snapshot(full);
        assert_eq!(b.locals[&room].text.text, "survives reconnect");
        assert_eq!(b.room, Some(room));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn multi_machine_down_send_keeps_draft_without_queue_or_replay() {
        let root = crate::utils::test_temp::unique_temp_path("multi-machine-down-send");
        let (mut ui, room) = fixture();
        ui.enable_client_state_for(&root, "remote", "reader")
            .unwrap();
        ui.set_machine("remote".into(), true);
        ui.locals
            .get_mut(&room)
            .unwrap()
            .text
            .insert("do this once");
        ui.text_changed(room);
        ui.set_machine_available(false);
        ui.request_send(room);
        assert_eq!(ui.locals[&room].text.text, "do this once");
        assert!(ui.pending.is_empty());
        assert!(ui.submissions.is_empty());
        ui.set_machine_available(true);
        ui.tick();
        assert!(ui.pending.is_empty());
        assert_eq!(ui.locals[&room].text.text, "do this once");
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn multi_machine_inherited_terminal_session_does_not_share_client_identity() {
        let a = identity_parts(None, Some("terminal-session"), Some("/dev/ttys001"), 1);
        let b = identity_parts(None, Some("terminal-session"), Some("/dev/ttys002"), 2);
        assert_ne!(a, b);
        assert_eq!(
            a,
            identity_parts(None, Some("terminal-session"), Some("/dev/ttys001"), 3)
        );
        assert_eq!(
            identity_parts(Some("explicit"), None, None, 1),
            identity_parts(Some("explicit"), None, None, 2)
        );
    }

    #[test]
    fn multi_machine_proxy_quit_only_saves_client_and_never_queues_shutdown() {
        let root = crate::utils::test_temp::unique_temp_path("multi-machine-proxy-quit");
        let (mut ui, room) = fixture();
        ui.enable_client_state_for(&root, "remote", "reader")
            .unwrap();
        ui.owns_coordinator = false;
        ui.locals
            .get_mut(&room)
            .unwrap()
            .text
            .insert("save on quit");
        ui.text_changed(room);
        ui.request_quit();
        ui.settle();
        assert!(ui.exit_ready);
        assert!(
            ui.pending.is_empty(),
            "proxy quit cannot stop the remote coordinator"
        );
        let (mut restored, _) = fixture();
        restored
            .enable_client_state_for(&root, "remote", "reader")
            .unwrap();
        assert_eq!(restored.locals[&room].text.text, "save on quit");
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn multi_machine_read_watermark_survives_a_pruned_snapshot() {
        let root = crate::utils::test_temp::unique_temp_path("multi-machine-seen-prune");
        let (mut ui, room) = fixture();
        ui.enable_client_state_for(&root, "remote", "reader")
            .unwrap();
        let mut latest = (*ui.snapshot).clone();
        let author = latest.state.agents().next().unwrap().id;
        latest
            .state
            .post_to_developer(room, author, "new report".into(), vec![], 42)
            .unwrap();
        ui.receive_snapshot(Arc::new(latest));
        assert_eq!(ui.client_state.as_ref().unwrap().seen[&room], 42);
        let (pruned, _) = fixture();
        ui.receive_snapshot(pruned.snapshot);
        assert_eq!(ui.client_state.as_ref().unwrap().seen[&room], 42);
        ui.open_room(room);
        assert_eq!(ui.client_state.as_ref().unwrap().seen[&room], 42);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn multi_machine_same_timestamp_messages_remain_unread_across_restart() {
        let root = crate::utils::test_temp::unique_temp_path("multi-machine-seen-tie");
        let (mut ui, room) = fixture();
        ui.enable_client_state_for(&root, "remote", "reader")
            .unwrap();
        let mut next = (*ui.snapshot).clone();
        let author = next.state.agents().next().unwrap().id;
        next.state
            .post_to_developer(room, author, "first".into(), vec![], 42)
            .unwrap();
        ui.receive_snapshot(Arc::new(next.clone()));
        assert!(!ui.room_unread(room));
        ui.set_client_focused(false);
        next.state
            .post_to_developer(room, author, "second in same poll".into(), vec![], 42)
            .unwrap();
        ui.receive_snapshot(Arc::new(next.clone()));
        assert!(ui.room_unread(room));

        let mut restored = BusUi::new(Arc::new(next));
        restored
            .enable_client_state_for(&root, "remote", "reader")
            .unwrap();
        assert!(
            restored.room_unread(room),
            "saved timestamp must not hide a different message"
        );
        restored.open_room(room);
        assert!(!restored.room_unread(room));
        assert_eq!(
            restored.client_state.as_ref().unwrap().seen_ids[&room].len(),
            2
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn multi_machine_retargeting_a_label_does_not_reuse_another_hosts_draft() {
        let root = crate::utils::test_temp::unique_temp_path("multi-machine-retarget");
        let (mut old, room) = fixture();
        old.set_machine("work".into(), true);
        old.enable_client_state_for(&root, "ssh:user@old-host", "reader")
            .unwrap();
        old.locals
            .get_mut(&room)
            .unwrap()
            .text
            .insert("only for old host");
        old.text_changed(room);

        let (mut replacement, same_id) = fixture();
        replacement.set_machine("work".into(), true);
        replacement
            .enable_client_state_for(&root, "ssh:user@new-host", "reader")
            .unwrap();
        assert_eq!(same_id, room);
        assert!(replacement.locals[&room].text.text.is_empty());

        let (mut renamed, _) = fixture();
        renamed.set_machine("renamed".into(), true);
        renamed
            .enable_client_state_for(&root, "ssh:user@old-host", "reader")
            .unwrap();
        assert_eq!(renamed.locals[&room].text.text, "only for old host");
        std::fs::remove_dir_all(root).unwrap();
    }
}
