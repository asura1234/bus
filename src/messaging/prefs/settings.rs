//! User preferences shared by every local Bus session.
//!
//! Choices that are not tied to one room live here, so a change made in any
//! Bus is what every Bus launched afterwards starts with: color blind mode,
//! MASTER's sound (every session has MASTER), the last All rooms sound, and
//! the worker compaction limit. A room's own sound stays in its session.
//! Sounds are read at launch and room creation; the coordinator reads the
//! compaction limit each tick so settings changes can re-arm notices.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::messaging::storage::{io, sessions as local_sessions};

pub(crate) const MIN_COMPACTIONS_PER_AGENT: u32 = 1;
pub(crate) const MAX_COMPACTIONS_PER_AGENT: u32 = 100;
pub(crate) const MAX_BACKGROUND_SHELL_HOLD_MINUTES: u32 = 24 * 60;
/// The sidebar width before it was adjustable, so unchanged settings look
/// the same as before.
pub(crate) const DEFAULT_SIDEBAR_WIDTH: u16 = 28;
/// Narrower would leave an agent name next to `Not ready ×` no room at all.
pub(crate) const MIN_SIDEBAR_WIDTH: u16 = 20;
/// Wider sidebars only pay off on very wide terminals; the layout also caps
/// the sidebar at a third of the terminal, so the chat keeps two thirds.
pub(crate) const MAX_SIDEBAR_WIDTH: u16 = 60;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct BusSettings {
    pub(crate) color_blind_mode: bool,
    pub(crate) feishu_notifications: bool,
    /// Internal enable generation, so sessions offline during off/on skip old history.
    pub(crate) feishu_notifications_generation: u64,
    /// MASTER's sound in every session; it rings by default.
    pub(crate) master_sound: SoundPref,
    /// The last All rooms choice: set on every work room at once, and what
    /// each new work room starts with.
    pub(crate) room_sound: SoundPref,
    /// Notify the room when a worker reaches this many compactions.
    #[serde(deserialize_with = "deserialize_compaction_limit")]
    pub(crate) max_compactions_per_agent: u32,
    /// How long a Claude reply waits for background shells its own turn
    /// started; then the latest interim text is delivered as the reply.
    #[serde(deserialize_with = "deserialize_shell_hold_minutes")]
    pub(crate) background_shell_hold_minutes: u32,
    /// The room sidebar's width in cells; the layout clamps it again on a
    /// narrow terminal.
    #[serde(deserialize_with = "deserialize_sidebar_width")]
    pub(crate) sidebar_width: u16,
}

impl Default for BusSettings {
    fn default() -> Self {
        Self {
            color_blind_mode: false,
            feishu_notifications: false,
            feishu_notifications_generation: 0,
            master_sound: SoundPref {
                enabled: true,
                name: None,
            },
            room_sound: SoundPref::default(),
            max_compactions_per_agent: 5,
            background_shell_hold_minutes: 15,
            sidebar_width: DEFAULT_SIDEBAR_WIDTH,
        }
    }
}

/// A hand-edited or out-of-date width is brought into range rather than
/// making the whole settings file unreadable.
pub(crate) fn clamp_sidebar_width(value: u16) -> u16 {
    value.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH)
}

fn deserialize_sidebar_width<'de, D>(deserializer: D) -> Result<u16, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    Ok(clamp_sidebar_width(
        u16::try_from(value).unwrap_or(u16::MAX),
    ))
}

pub(crate) fn validate_compaction_limit(value: u32) -> Result<(), String> {
    if (MIN_COMPACTIONS_PER_AGENT..=MAX_COMPACTIONS_PER_AGENT).contains(&value) {
        Ok(())
    } else {
        Err("Max compactions per agent must be between 1 and 100.".into())
    }
}

fn deserialize_compaction_limit<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = u32::deserialize(deserializer)?;
    validate_compaction_limit(value).map_err(serde::de::Error::custom)?;
    Ok(value)
}

fn deserialize_shell_hold_minutes<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = u32::deserialize(deserializer)?;
    if (1..=MAX_BACKGROUND_SHELL_HOLD_MINUTES).contains(&value) {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(
            "background_shell_hold_minutes must be between 1 and 1440",
        ))
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
        if let Ok(base) = local_sessions::default_base_dir() {
            return Some(base.join("settings.json"));
        }
    }
    crate::utils::env::bus_data_dir().map(|root| root.join("settings.json"))
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
    io::private_dir(parent).map_err(|error| failed(&error))?;
    let _lease =
        io::append_lock(&path.with_extension("json.lock")).map_err(|error| failed(&error))?;
    let mut settings = load(path).unwrap_or_default();
    change(&mut settings);
    save(path, &settings)?;
    Ok(settings)
}

pub(crate) fn save(path: &Path, settings: &BusSettings) -> Result<(), String> {
    validate_compaction_limit(settings.max_compactions_per_agent)?;
    let failed = |error: &dyn std::fmt::Display| {
        format!("Could not save Bus settings to {}: {error}", path.display())
    };
    let parent = path.parent().ok_or_else(|| failed(&"missing parent"))?;
    io::private_dir(parent).map_err(|error| failed(&error))?;
    let bytes = serde_json::to_vec_pretty(&settings).map_err(|error| failed(&error))?;
    io::atomic_write(path, &bytes).map_err(|error| failed(&error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feishu_notifications_default_off_without_a_recipient_setting() {
        let value = serde_json::to_value(BusSettings::default()).unwrap();
        assert_eq!(value["feishu_notifications"], false);
        assert!(value.get("feishu_recipient").is_none());
        assert!(value.get("feishu_user_id").is_none());
        assert!(value.get("feishu_chat_id").is_none());
    }

    #[test]
    fn compaction_limit_defaults_to_five_and_validates_config_values() {
        let root = crate::utils::test_temp::unique_temp_path("bus-compaction-settings");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("settings.json");
        let defaults = serde_json::to_value(load(&path).unwrap()).unwrap();
        assert_eq!(defaults["max_compactions_per_agent"], 5);
        for value in [1, 5, 100] {
            std::fs::write(&path, format!("{{\"max_compactions_per_agent\":{value}}}")).unwrap();
            let loaded = load(&path).unwrap();
            assert_eq!(
                serde_json::to_value(&loaded).unwrap()["max_compactions_per_agent"],
                value
            );
            save(&path, &loaded).unwrap();
            assert_eq!(load(&path).unwrap(), loaded);
        }
        for value in ["0", "101", "-1", "1.5", "\"five\""] {
            std::fs::write(&path, format!("{{\"max_compactions_per_agent\":{value}}}")).unwrap();
            assert!(load(&path).is_err(), "invalid limit {value}");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sidebar_width_defaults_to_today_and_clamps_config_values() {
        let root = crate::utils::test_temp::unique_temp_path("bus-sidebar-width-settings");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("settings.json");
        assert_eq!(BusSettings::default().sidebar_width, DEFAULT_SIDEBAR_WIDTH);
        assert_eq!(DEFAULT_SIDEBAR_WIDTH, 28);
        assert_eq!(
            serde_json::to_value(load(&path).unwrap()).unwrap()["sidebar_width"],
            28
        );
        for (written, loaded) in [(20, 20), (45, 45), (60, 60), (5, 20), (500, 60), (0, 20)] {
            std::fs::write(&path, format!("{{\"sidebar_width\":{written}}}")).unwrap();
            assert_eq!(load(&path).unwrap().sidebar_width, loaded, "{written}");
        }
        assert_eq!(clamp_sidebar_width(19), MIN_SIDEBAR_WIDTH);
        assert_eq!(clamp_sidebar_width(61), MAX_SIDEBAR_WIDTH);
        assert_eq!(clamp_sidebar_width(33), 33);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn color_blind_mode_defaults_off_and_round_trips() {
        let root = std::env::temp_dir().join(format!(
            "bus-settings-{}-{}",
            std::process::id(),
            io::now_ns()
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
    fn background_shell_hold_defaults_to_fifteen_minutes_within_its_range() {
        let root = std::env::temp_dir().join(format!(
            "bus-settings-hold-{}-{}",
            std::process::id(),
            io::now_ns()
        ));
        let path = root.join("settings.json");
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(BusSettings::default().background_shell_hold_minutes, 15);
        std::fs::write(&path, br#"{"background_shell_hold_minutes":2}"#).unwrap();
        assert_eq!(load(&path).unwrap().background_shell_hold_minutes, 2);
        for invalid in ["0", "1441"] {
            std::fs::write(
                &path,
                format!(r#"{{"background_shell_hold_minutes":{invalid}}}"#),
            )
            .unwrap();
            assert!(load(&path).is_err(), "{invalid}");
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sound_choices_default_to_today_and_update_keeps_other_fields() {
        let root = std::env::temp_dir().join(format!(
            "bus-settings-sound-{}-{}",
            std::process::id(),
            io::now_ns()
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
        let _guard = crate::utils::config::test_config_env_lock()
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
                local_sessions::default_base_dir()
                    .unwrap()
                    .join("settings.json")
            )
        );
    }
}
