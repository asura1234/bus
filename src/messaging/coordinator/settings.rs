//! Global settings the coordinator applies to its session: MASTER's sound and
//! the All rooms sound, which new work rooms start with (see `bus::settings`).
use super::{mpsc, BusEvent, BusState, Room, RoomId, RoomKind, Worker};
#[cfg(test)]
use super::{BusCommand, Method, Path, PathBuf, ResponseResult, Transport};
use crate::bus::settings::{self, BusSettings, SoundPref};

impl Worker {
    /// At launch, MASTER takes the global sound.
    pub(super) fn apply_global_settings(&mut self) -> Result<(), String> {
        let Some(path) = self.settings_path.clone() else {
            return Ok(());
        };
        let Some(id) = self.state.master_room().map(|master| master.id) else {
            return Ok(());
        };
        let pref = settings::load(&path).unwrap_or_default().master_sound;
        let mut state = self.state.clone();
        state
            .set_room_sound(id, pref.enabled)
            .and_then(|()| state.set_room_sound_name(id, pref.name))
            .map_err(|e| e.to_string())?;
        self.save(state)
    }

    /// A new work room starts with the global All rooms sound.
    pub(super) fn apply_new_room_sound(&self, state: &mut BusState, room: RoomId) {
        let Some(path) = &self.settings_path else {
            return;
        };
        let pref = settings::load(path).unwrap_or_default().room_sound;
        let applied = state
            .set_room_sound(room, pref.enabled)
            .and_then(|()| state.set_room_sound_name(room, pref.name));
        if let Err(error) = applied {
            tracing::warn!(%error, "new room sound not applied");
        }
    }

    /// MASTER's sound changed in this session; it is global, so save it.
    pub(super) fn record_master_sound(&self, state: &BusState, room: RoomId) -> Result<(), String> {
        let (Some(path), Some(master)) = (&self.settings_path, state.master_room()) else {
            return Ok(());
        };
        if master.id != room {
            return Ok(());
        }
        let pref = room_sound(master);
        settings::update(path, |settings| settings.master_sound = pref).map(|_| ())
    }

    /// The All rooms sound: sets the changed part (on/off, the sound, or both)
    /// on every work room, and saves it as what new rooms start with. A room's
    /// other part stays, so a room can still differ afterwards.
    pub(super) fn set_all_rooms_sound(
        &mut self,
        enabled: Option<bool>,
        name: Option<Option<String>>,
        events: &mpsc::Sender<BusEvent>,
    ) -> Result<BusSettings, String> {
        // Checked first so a missing settings file changes no room.
        let path = self
            .settings_path
            .clone()
            .ok_or("Bus settings location unavailable")?;
        let mut state = self.state.clone();
        let rooms: Vec<RoomId> = state
            .rooms()
            .filter(|room| room.kind == RoomKind::Work)
            .map(|room| room.id)
            .collect();
        for room in rooms {
            if let Some(on) = enabled {
                state.set_room_sound(room, on).map_err(|e| e.to_string())?;
            }
            if let Some(name) = &name {
                state
                    .set_room_sound_name(room, name.clone())
                    .map_err(|e| e.to_string())?;
            }
        }
        self.save(state)?;
        let saved = settings::update(&path, |settings| {
            if let Some(on) = enabled {
                settings.room_sound.enabled = on;
            }
            if let Some(name) = name {
                settings.room_sound.name = name;
            }
        })?;
        let _ = events.send(BusEvent::SettingsChanged(saved.clone()));
        Ok(saved)
    }
}

/// A room's effective sound, as the Settings screen shows it.
fn room_sound(room: &Room) -> SoundPref {
    SoundPref {
        enabled: room.sound_enabled(),
        name: room.sound_name.clone(),
    }
}

#[cfg(test)]
#[path = "tests/settings_test.rs"]
mod tests;
