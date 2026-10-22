//! Durable, independent disk warning and critical episodes for every work room.
use super::{BusState, ModelError, RoomKind};
use serde::{Deserialize, Serialize};

pub(crate) const BYTES_PER_GB: u64 = 1_000_000_000;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub(super) struct DiskAlerts {
    warning_sent: bool,
    critical_sent: bool,
}

impl DiskAlerts {
    pub(super) fn is_empty(&self) -> bool {
        !self.warning_sent && !self.critical_sent
    }
}

fn latch(sent: bool, bytes: u64, threshold_gb: u32) -> bool {
    let threshold = u64::from(threshold_gb) * BYTES_PER_GB;
    if bytes < threshold {
        true
    } else if bytes > threshold + BYTES_PER_GB {
        false
    } else {
        sent
    }
}

impl BusState {
    pub(crate) fn notify_disk_space(
        &mut self,
        bytes: u64,
        warning: u32,
        critical: u32,
        now_ms: u64,
    ) -> Result<bool, ModelError> {
        let rooms: Vec<_> = self
            .rooms
            .values()
            .filter(|room| room.kind == RoomKind::Work && !room.deletion_pending)
            .map(|room| (room.id, room.disk_alerts))
            .collect();
        let gb = bytes / BYTES_PER_GB;
        let mut changed = false;
        for (room, before) in rooms {
            let next = DiskAlerts {
                warning_sent: latch(before.warning_sent, bytes, warning),
                critical_sent: latch(before.critical_sent, bytes, critical),
            };
            let recipient = self.notice_orchestrator(room, None);
            let critical_crossed = next.critical_sent && !before.critical_sent;
            if next.warning_sent && !before.warning_sent && !critical_crossed {
                self.post_bus_notice(room, recipient, format!(
                    "Disk low: {gb} GB free. Delete finished worktrees and cargo target dirs before starting new builds."
                ), now_ms)?;
            }
            if critical_crossed {
                self.post_bus_notice(room, recipient, format!(
                    "URGENT: disk almost full ({gb} GB free). Stop starting builds now and free space immediately: delete merged worktrees and target dirs."
                ), now_ms)?;
            }
            if next != before {
                self.rooms
                    .get_mut(&room)
                    .ok_or(ModelError::UnknownRoom(room))?
                    .disk_alerts = next;
                changed = true;
            }
        }
        Ok(changed)
    }
}
