//! Messaging perf metrics; they run in the client process and land in `perf/client.jsonl`.
use crate::utils::perf::{LoopWatch, Timer};

/// One coordinator pass: a command, a control call, a poll tick or a snapshot.
pub(crate) static COORDINATOR_LOOP: LoopWatch = LoopWatch::new("coordinator.loop");
/// Applying one room UI command; slow ones are logged with the command.
pub(crate) static COMMAND: Timer = Timer::new("coordinator.command");
/// Executing one control call on the coordinator; slow ones are logged with the method.
pub(crate) static CONTROL: Timer = Timer::new("coordinator.control");
/// One agent status poll and delivery pass.
pub(crate) static TICK: Timer = Timer::new("coordinator.tick");
/// Building the snapshot the room UI reads.
pub(crate) static SNAPSHOT: Timer = Timer::new("coordinator.snapshot");
/// A control socket request from dispatch to reply, including its wait in the command queue.
pub(crate) static CONTROL_REQUEST: Timer = Timer::new("control.request");
/// One call from the coordinator to the server API; slow ones are logged with the method.
pub(crate) static SERVER_API: Timer = Timer::new("coordinator.server_api");
/// Writing `state.json`, including its fsyncs.
pub(crate) static STORE_SAVE: Timer = Timer::new("store.save");
/// Appending to `state.journal.json`, including its fsyncs.
pub(crate) static STORE_JOURNAL: Timer = Timer::new("store.journal");

/// The command's variant, never its payload: drafts and notes are user content.
pub(crate) fn command_name(command: &super::coordinator::BusCommand) -> &'static str {
    use super::coordinator::BusCommand as C;
    match command {
        C::CreateRoom(_) => "create_room",
        C::RenameRoom(..) => "rename_room",
        C::SetRoomSound(..) => "set_room_sound",
        C::SetRoomSoundName(..) => "set_room_sound_name",
        C::SetAllRoomsSound(_) => "set_all_rooms_sound",
        C::SetAllRoomsSoundName(_) => "set_all_rooms_sound_name",
        C::SetMaxCompactionsPerAgent(_) => "set_max_compactions_per_agent",
        C::SetFreezeNotifications(_) => "set_freeze_notifications",
        C::SetFeishuRecipient(_) => "set_feishu_recipient",
        C::SetFeishuCliPath(_) => "set_feishu_cli_path",
        C::SetSidebarWidth(_) => "set_sidebar_width",
        C::SetFeishuNotifications(_) => "set_feishu_notifications",
        C::RenameAgent(..) => "rename_agent",
        C::DeleteRoom(_) => "delete_room",
        C::DeleteAgent(_) => "delete_agent",
        C::SelectRoom(_) => "select_room",
        C::LeaveRoom => "leave_room",
        C::MarkRoomSeen(_) => "mark_room_seen",
        C::SetNotes(..) => "set_notes",
        C::SetDraftText(..) => "set_draft_text",
        C::SetRecipients(..) => "set_recipients",
        C::AttachFile(..) => "attach_file",
        C::RemoveFile(..) => "remove_file",
        C::Submit(_) => "submit",
        C::SubmitQueued(_) => "submit_queued",
        C::SetDetails(..) => "set_details",
        C::AddAgent(_) => "add_agent",
        C::AddOrchestrator(..) => "add_orchestrator",
        C::FocusTerminal(_) => "focus_terminal",
        C::CompleteHookSetup(_) => "complete_hook_setup",
        C::Suggestions { .. } => "suggestions",
        C::Control(_) => "control",
        C::Shutdown => "shutdown",
    }
}

/// A control method's name for perf logs: a known method's static name, else a
/// fixed label, since the method field is whatever the caller sent.
pub(crate) fn control_method_label(method: &str) -> &'static str {
    super::coordinator::method_spec(method).map_or("unknown_method", |spec| spec.method)
}

pub(crate) static STORE_SERIALIZE: Timer = Timer::new("coordinator.store.serialize");
pub(crate) static STORE_WRITE: Timer = Timer::new("coordinator.store.write");
pub(crate) static STORE_FSYNC: Timer = Timer::new("coordinator.store.fsync");
pub(crate) static STORE_RENAME: Timer = Timer::new("coordinator.store.rename");
pub(crate) static STORE_DIR_SYNC: Timer = Timer::new("coordinator.store.dir_sync");
pub(crate) static STORE_BYTES: crate::utils::perf::Counter =
    crate::utils::perf::Counter::new("coordinator.store.bytes");
