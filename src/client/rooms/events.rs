use super::BusUi;
use crate::messaging::{
    coordinator::BusCommand,
    model::{AgentId, RoomId},
};

impl BusUi {
    /// The coordinator echoes the saved settings for every stepper press
    /// before that press's command is finished, so a quick second press
    /// would be undone by the first press's echo. A stepper value whose
    /// command is still pending stays as shown; the final echo matches it.
    pub(in crate::client::rooms) fn receive_settings(
        &mut self,
        mut settings: crate::messaging::prefs::settings::BusSettings,
    ) {
        let pending = |is: fn(&BusCommand) -> bool| {
            self.pending
                .iter()
                .any(|p| p.result.is_none() && is(&p.command))
        };
        if pending(|command| matches!(command, BusCommand::SetSidebarWidth(_))) {
            settings.sidebar_width = self.settings.sidebar_width;
        }
        if pending(|command| matches!(command, BusCommand::SetFeishuNotifications(_))) {
            settings.feishu_notifications = self.settings.feishu_notifications;
        }
        if pending(|command| matches!(command, BusCommand::SetDeveloperSoundName(_))) {
            settings.developer_sound = self.settings.developer_sound.clone();
        }
        if pending(|command| matches!(command, BusCommand::SetDeveloperSoundEnabled(_))) {
            settings.developer_sound_enabled = self.settings.developer_sound_enabled;
        }
        self.settings = settings;
    }

    /// The coordinator re-reads the shared settings each tick, so its Feishu
    /// status carries a change made by another Bus session. Mirror the live
    /// toggle unless this client's own toggle is still awaiting its result.
    pub(in crate::client::rooms) fn receive_feishu_status(
        &mut self,
        status: crate::messaging::coordinator::attention::Status,
    ) {
        let pending = self.pending.iter().any(|p| {
            p.result.is_none() && matches!(p.command, BusCommand::SetFeishuNotifications(_))
        });
        if !pending {
            self.settings.feishu_notifications = status.enabled;
        }
        self.feishu_status = status;
    }

    pub(in crate::client::rooms) fn receive_dev_focus(
        &mut self,
        room: RoomId,
        agent: Option<AgentId>,
        form_generation: Option<u64>,
    ) {
        if (self.form.is_some() || self.rename.is_some())
            && form_generation != Some(self.form_generation)
        {
            return;
        }
        if let Some(agent) = agent {
            // Show this agent's room in the sidebar without marking the
            // room history read merely because its terminal was opened.
            self.room = Some(room);
            self.open_terminal(agent);
        } else {
            self.open_room(room);
        }
    }
}
