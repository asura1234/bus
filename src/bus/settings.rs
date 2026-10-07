//! User preferences shared by every local Bus session.
//!
//! Choices that are not tied to one room live here, so a change made in any
//! Bus is what every Bus launched afterwards starts with: color blind mode,
//! MASTER's sound (every session has MASTER), and the sound a new work room
//! starts with. A room's own sound stays in its session. Running Bus
//! instances read this file at launch and when they create a room; they do
//! not follow another instance's changes live.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct BusSettings {
    pub(crate) color_blind_mode: bool,
    /// MASTER's sound in every session; it rings by default.
    pub(crate) master_sound: SoundPref,
    /// What each new work room starts with.
    pub(crate) room_sound: SoundPref,
}

impl Default for BusSettings {
    fn default() -> Self {
        Self {
            color_blind_mode: false,
            master_sound: SoundPref {
                enabled: true,
                name: None,
            },
            room_sound: SoundPref::default(),
        }
    }
}

/// A sound notification choice: on or off, and the system sound by name
/// (None is Bus's own ding).
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct SoundPref {
    pub(crate) enabled: bool,
    pub(crate) name: Option<String>,
}

/// Registry sessions share one file beside the registry; an explicit
/// `BUS_DATA_DIR` root (tests, e2e, isolated development copies) stays
/// isolated with its own `settings.json` in that root.
pub(crate) fn path() -> Option<PathBuf> {
    if std::env::var_os("BUS_SESSION_ID").is_some() {
        if let Ok(base) = super::local_sessions::default_base_dir() {
            return Some(base.join("settings.json"));
        }
    }
    super::entry::data_dir().map(|root| root.join("settings.json"))
}

pub(crate) fn load(path: &Path) -> Result<BusSettings, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
            format!(
                "Bus settings at {} are unreadable; using defaults: {error}",
                path.display()
            )
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BusSettings::default()),
        Err(error) => Err(format!(
            "Bus settings at {} are unreadable; using defaults: {error}",
            path.display()
        )),
    }
}

/// Changes the saved settings under a lock, so concurrent writers (another Bus,
/// or this one's UI and coordinator) each keep the fields they did not change.
/// A file that does not parse is replaced from defaults, as `load` reports.
pub(crate) fn update(
    path: &Path,
    change: impl FnOnce(&mut BusSettings),
) -> Result<BusSettings, String> {
    let failed = |error: &dyn std::fmt::Display| {
        format!("Could not save Bus settings to {}: {error}", path.display())
    };
    let parent = path.parent().ok_or_else(|| failed(&"missing parent"))?;
    super::io::private_dir(parent).map_err(|error| failed(&error))?;
    let _lease = super::io::append_lock(&path.with_extension("json.lock"))
        .map_err(|error| failed(&error))?;
    let mut settings = load(path).unwrap_or_default();
    change(&mut settings);
    save(path, &settings)?;
    Ok(settings)
}

pub(crate) fn save(path: &Path, settings: &BusSettings) -> Result<(), String> {
    let failed = |error: &dyn std::fmt::Display| {
        format!("Could not save Bus settings to {}: {error}", path.display())
    };
    let parent = path.parent().ok_or_else(|| failed(&"missing parent"))?;
    super::io::private_dir(parent).map_err(|error| failed(&error))?;
    let bytes = serde_json::to_vec_pretty(&settings).map_err(|error| failed(&error))?;
    super::io::atomic_write(path, &bytes).map_err(|error| failed(&error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_blind_mode_defaults_off_and_round_trips() {
        let root = std::env::temp_dir().join(format!(
            "bus-settings-{}-{}",
            std::process::id(),
            super::super::io::now_ns()
        ));
        let path = root.join("nested").join("settings.json");
        assert_eq!(load(&path), Ok(BusSettings::default()));
        assert!(!BusSettings::default().color_blind_mode);

        let enabled = BusSettings {
            color_blind_mode: true,
            ..BusSettings::default()
        };
        save(&path, &enabled).unwrap();
        assert_eq!(load(&path), Ok(enabled.clone()));

        // Settings written by a newer Bus keep the fields this one knows.
        std::fs::write(&path, br#"{"color_blind_mode":true,"future":1}"#).unwrap();
        assert_eq!(load(&path), Ok(enabled));
        std::fs::write(&path, b"not json").unwrap();
        assert!(load(&path).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sound_choices_default_to_today_and_update_keeps_other_fields() {
        let root = std::env::temp_dir().join(format!(
            "bus-settings-sound-{}-{}",
            std::process::id(),
            super::super::io::now_ns()
        ));
        let path = root.join("settings.json");
        std::fs::create_dir_all(&root).unwrap();
        // Unset sounds default: MASTER rings and new rooms start silent, both
        // with Bus's ding.
        std::fs::write(&path, br#"{"color_blind_mode":true}"#).unwrap();
        let old = load(&path).unwrap();
        assert!(old.color_blind_mode);
        assert!(old.master_sound.enabled);
        assert_eq!(old.master_sound.name, None);
        assert_eq!(old.room_sound, SoundPref::default());
        assert!(!old.room_sound.enabled);

        let glass = SoundPref {
            enabled: true,
            name: Some("Glass".into()),
        };
        let saved = update(&path, |settings| settings.room_sound = glass.clone()).unwrap();
        assert!(saved.color_blind_mode);
        let saved = update(&path, |settings| {
            settings.master_sound = SoundPref {
                enabled: false,
                name: None,
            }
        })
        .unwrap();
        assert_eq!(saved.room_sound, glass);
        assert_eq!(load(&path), Ok(saved));

        // An unreadable file is replaced from defaults rather than blocking a change.
        std::fs::write(&path, b"not json").unwrap();
        let fresh = update(&path, |settings| settings.color_blind_mode = true).unwrap();
        assert_eq!(fresh.room_sound, SoundPref::default());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn registry_sessions_share_one_file_and_an_explicit_data_dir_stays_isolated() {
        let _guard = crate::config::test_config_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous: Vec<_> = ["BUS_DATA_DIR", "BUS_SESSION_ID"]
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        let root = std::env::temp_dir().join("bus-settings-isolated-root");
        std::env::set_var("BUS_DATA_DIR", &root);
        std::env::remove_var("BUS_SESSION_ID");
        let isolated = path();
        std::env::set_var("BUS_SESSION_ID", "0123456789abcdef");
        let shared = path();
        for (key, value) in previous {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
        assert_eq!(isolated, Some(root.join("settings.json")));
        assert_eq!(
            shared,
            Some(
                super::super::local_sessions::default_base_dir()
                    .unwrap()
                    .join("settings.json")
            )
        );
    }
}
