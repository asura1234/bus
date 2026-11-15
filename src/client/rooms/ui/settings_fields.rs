//! Settings focus indexes. Each stepper sits one row after the previous one.
use super::BusUi;

impl BusUi {
    /// The compaction limit's Settings focus index, after the sound rows.
    pub(in crate::client::rooms) fn compaction_field(&self) -> usize {
        self.room_sound_field(self.sound_settings_targets().len())
    }

    pub(in crate::client::rooms) fn idle_agents_field(&self) -> usize {
        self.compaction_field() + 1
    }

    pub(in crate::client::rooms) fn idle_alert_minutes_field(&self) -> usize {
        self.idle_agents_field() + 1
    }

    pub(in crate::client::rooms) fn disk_warning_field(&self) -> usize {
        self.idle_alert_minutes_field() + 1
    }

    pub(in crate::client::rooms) fn disk_critical_field(&self) -> usize {
        self.disk_warning_field() + 1
    }

    /// The sidebar width's Settings focus index, after the disk thresholds.
    pub(in crate::client::rooms) fn sidebar_width_field(&self) -> usize {
        self.disk_critical_field() + 1
    }
}
