//! Dev variant only: names every clickable region of the frame just laid out
//! and hands them to the coordinator's `ui hits`, so the TUI driver can click
//! `room:review` or `new-agent` instead of guessing cells.
use super::super::deletion::DeleteTarget;
use super::super::BusUi;
use super::{Action, SoundTarget};
use crate::messaging::coordinator::ui_hits::{publish, UiHit};
use crate::messaging::model::{AgentId, Provider, RoomId};

impl BusUi {
    pub(super) fn publish_hits(&self, cols: u16, rows: u16) {
        let hits = self
            .view
            .hits
            .iter()
            .map(|hit| UiHit {
                name: self.hit_name(&hit.action),
                row: hit.rect.y,
                col: hit.rect.x,
                width: hit.rect.width,
                height: hit.rect.height,
            })
            .collect();
        publish(cols, rows, hits);
    }

    fn room_name(&self, id: RoomId) -> String {
        self.snapshot
            .state
            .room(id)
            .map_or_else(|| id.0.to_string(), |room| room.name.clone())
    }

    fn agent_name(&self, id: AgentId) -> String {
        self.snapshot
            .state
            .agent(id)
            .map_or_else(|| id.0.to_string(), |agent| agent.name.clone())
    }

    fn sound_target(&self, target: &SoundTarget) -> String {
        match target {
            SoundTarget::Room(id) => self.room_name(*id),
            SoundTarget::AllRooms => "all-rooms".into(),
        }
    }

    fn hit_name(&self, action: &Action) -> String {
        let file = |path: &std::path::Path| {
            path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            )
        };
        let way = |up: bool| if up { "up" } else { "down" };
        match action {
            Action::Delete(DeleteTarget::Room(id)) => {
                format!("delete-room:{}", self.room_name(*id))
            }
            Action::Delete(DeleteTarget::Agent(id)) => {
                format!("delete-agent:{}", self.agent_name(*id))
            }
            Action::CancelDelete => "cancel-delete".into(),
            Action::ConfirmDelete => "confirm-delete".into(),
            Action::DismissAlert => "dismiss-alert".into(),
            Action::Room(id) => format!("room:{}", self.room_name(*id)),
            Action::Agent(id) => format!("agent:{}", self.agent_name(*id)),
            Action::NewRoom => "new-room".into(),
            Action::NewAgent => "new-agent".into(),
            Action::ScrollFiles(up) => format!("scroll-files:{}", way(*up)),
            Action::Notes => "notes".into(),
            Action::Composer => "composer".into(),
            Action::Recipients => "recipients".into(),
            Action::Recipient(Some(id)) => format!("recipient:{}", self.agent_name(*id)),
            Action::Recipient(None) => "recipient:none".into(),
            Action::Files => "files".into(),
            Action::RemoveFile(path) => format!("remove-file:{}", file(path)),
            Action::OpenFile(path) => format!("file:{}", file(path)),
            Action::Details(id) => format!("details:{}", self.agent_name(*id)),
            Action::Quote(id) => format!("quote:{}", id.0),
            Action::Field(index) => format!("field:{index}"),
            // The names `agent add --provider` takes.
            Action::Provider(Provider::ClaudeCode) => "provider:claude".into(),
            Action::Provider(Provider::Codex) => "provider:codex".into(),
            Action::Provider(Provider::Cursor) => "provider:cursor".into(),
            Action::Orchestrates => "orchestrates".into(),
            Action::Suggestion(index) => format!("suggestion:{index}"),
            Action::Settings => "settings".into(),
            Action::ToggleColorBlindMode => "color-blind".into(),
            Action::TogglePerfOverlay => "perf-overlay".into(),
            #[cfg(feature = "dev-tools")]
            Action::SimulateFreeze => "freeze-test".into(),
            Action::ToggleFreezeNotifications => "freeze-notifications".into(),
            Action::EditFeishuRecipient => "feishu-recipient".into(),
            Action::EditFeishuCli => "feishu-cli-path".into(),
            Action::ToggleFeishuNotifications => "feishu-notifications".into(),
            Action::ToggleSound(target) => format!("sound:{}", self.sound_target(target)),
            Action::CycleSound(target, next) => format!(
                "sound-{}:{}",
                if *next { "next" } else { "previous" },
                self.sound_target(target)
            ),
            Action::AdjustCompactionLimit(up) => format!("compaction-limit:{}", way(*up)),
            Action::AdjustIdleAgentsAlert(up) => format!("idle-agents-alert:{}", way(*up)),
            Action::AdjustDiskWarningGb(up) => format!("disk-warning:{}", way(*up)),
            Action::AdjustDiskCriticalGb(up) => format!("disk-critical:{}", way(*up)),
            Action::AdjustSidebarWidth(up) => format!("sidebar-width:{}", way(*up)),
            Action::Cancel => "cancel".into(),
            Action::Add => "add".into(),
        }
    }
}
