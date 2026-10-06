//! Global settings the coordinator applies to its session: MASTER's sound and
//! the sound each new work room starts with (see `bus::settings`).
use super::*;
use crate::bus::settings::{self, BusSettings, SoundPref};

impl Worker {
    /// At launch, MASTER takes the global sound. The first launch after an
    /// upgrade records this session's MASTER sound as the global one instead.
    pub(super) fn apply_global_settings(&mut self) -> Result<(), String> {
        let Some(path) = self.settings_path.clone() else {
            return Ok(());
        };
        let Some(master) = self.state.master_room() else {
            return Ok(());
        };
        let (id, current) = (master.id, room_sound(master));
        let saved = settings::update(&path, |settings| {
            settings.master_sound.get_or_insert(current);
        })?;
        let pref = saved.master_sound.unwrap_or_default();
        let mut state = self.state.clone();
        state
            .set_room_sound(id, pref.enabled)
            .and_then(|()| state.set_room_sound_name(id, pref.name))
            .map_err(|e| e.to_string())?;
        self.save(state)
    }

    /// A new work room starts with the global new-room sound.
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
        settings::update(path, |settings| settings.master_sound = Some(pref)).map(|_| ())
    }

    /// Changes the global new-room sound and tells the UI.
    pub(super) fn update_new_room_sound(
        &self,
        change: impl FnOnce(&mut SoundPref),
        events: &mpsc::Sender<BusEvent>,
    ) -> Result<BusSettings, String> {
        let path = self
            .settings_path
            .as_ref()
            .ok_or("Bus settings location unavailable")?;
        let saved = settings::update(path, |settings| change(&mut settings.room_sound))?;
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
#[path = "runtime_settings_tests.rs"]
mod tests;
