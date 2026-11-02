//! User preferences shared by every local Bus session.
//!
//! Choices that are not tied to one room live here, so a change made in any
//! Bus is what every Bus launched afterwards starts with: color blind mode,
//! MASTER's sound (every session has MASTER), the last All rooms sound, and
//! the worker compaction, idle-agents and disk alert limits. A room's own sound stays in
//! its session. Sounds are read at launch and room creation; the coordinator
//! reads notice limits each tick and disk thresholds when a sample completes.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::messaging::storage::{io, sessions as local_sessions};

#[cfg(test)]
#[path = "tests/disk_settings_test.rs"]
mod disk_settings_tests;

#[cfg(test)]
#[path = "tests/developer_sound_test.rs"]
mod developer_sound_tests;

pub(crate) const MIN_COMPACTIONS_PER_AGENT: u32 = 1;
pub(crate) const MAX_COMPACTIONS_PER_AGENT: u32 = 100;
pub(crate) const MIN_IDLE_AGENTS_ALERT: u32 = 1;
pub(crate) const MAX_IDLE_AGENTS_ALERT: u32 = 10;
pub(crate) const MIN_DISK_FREE_GB: u32 = 1;
pub(crate) const MAX_DISK_FREE_GB: u32 = 10_000;
/// The sidebar width before it was adjustable, so unchanged settings look
/// the same as before.
pub(crate) const DEFAULT_SIDEBAR_WIDTH: u16 = 28;
/// Narrower would leave an agent name next to `Unavailable ×` no room at all.
pub(crate) const MIN_SIDEBAR_WIDTH: u16 = 20;
/// Wider sidebars only pay off on very wide terminals; the layout also caps
/// the sidebar at a third of the terminal, so the chat keeps two thirds.
pub(crate) const MAX_SIDEBAR_WIDTH: u16 = 60;

pub(crate) const DEFAULT_FEISHU_RECIPIENT: &str = "ou_ca1b530d792419a044e281f998ce68dc";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct BusSettings {
    pub(crate) color_blind_mode: bool,
    /// Dev-only target, resolved against current MASTER orchestrator names.
    pub(crate) error_orchestrator: Option<String>,
    // Read the legacy toggle for compatibility; developer alerts are mandatory.
    #[serde(
        rename = "feishu_waiting_notifications",
        deserialize_with = "mandatory_notifications"
    )]
    pub(crate) feishu_notifications: bool,
    pub(crate) feishu_recipient: String,
    pub(crate) feishu_cli_path: Option<String>,
    /// Installed developer-alert sound; None uses the system default.
    pub(crate) developer_sound: Option<String>,
    /// Default on for settings written before the sound had a checkbox.
    pub(crate) developer_sound_enabled: bool,
    /// MASTER's sound in every session; it rings by default.
    pub(crate) master_sound: SoundPref,
    /// The last All rooms choice: set on every work room at once, and what
    /// each new work room starts with.
    pub(crate) room_sound: SoundPref,
    /// Notify the room when a worker reaches this many compactions.
    #[serde(deserialize_with = "deserialize_compaction_limit")]
    pub(crate) max_compactions_per_agent: u32,
    /// Notify its orchestrator when more than this many workers have sat idle for 60 seconds.
    #[serde(deserialize_with = "deserialize_idle_agents_alert")]
    pub(crate) idle_agents_alert: u32,
    #[serde(deserialize_with = "deserialize_disk_free_gb")]
    pub(crate) disk_warning_gb: u32,
    #[serde(deserialize_with = "deserialize_disk_free_gb")]
    pub(crate) disk_critical_gb: u32,
    /// The room sidebar's width in cells; the layout clamps it again on a
    /// narrow terminal.
    #[serde(deserialize_with = "deserialize_sidebar_width")]
    pub(crate) sidebar_width: u16,
}

fn mandatory_notifications<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<bool, D::Error> {
    let _ = bool::deserialize(deserializer)?;
    Ok(true)
}

impl Default for BusSettings {
    fn default() -> Self {
        Self {
            color_blind_mode: false,
            error_orchestrator: None,
            feishu_notifications: true,
            feishu_recipient: DEFAULT_FEISHU_RECIPIENT.into(),
            feishu_cli_path: None,
            developer_sound: None,
            developer_sound_enabled: true,
            master_sound: SoundPref {
                enabled: true,
                name: None,
            },
            room_sound: SoundPref::default(),
            max_compactions_per_agent: 5,
            idle_agents_alert: 5,
            disk_warning_gb: 20,
            disk_critical_gb: 5,
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

pub(crate) fn validate_idle_agents_alert(value: u32) -> Result<(), String> {
    if (MIN_IDLE_AGENTS_ALERT..=MAX_IDLE_AGENTS_ALERT).contains(&value) {
        Ok(())
    } else {
        Err("Idle agents alert must be between 1 and 10.".into())
    }
}

fn deserialize_idle_agents_alert<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = u32::deserialize(deserializer)?;
    validate_idle_agents_alert(value).map_err(serde::de::Error::custom)?;
    Ok(value)
}

fn deserialize_disk_free_gb<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = u32::deserialize(deserializer)?;
    if !(MIN_DISK_FREE_GB..=MAX_DISK_FREE_GB).contains(&value) {
        return Err(serde::de::Error::custom(
            "Disk thresholds must be between 1 and 10000 GB.",
        ));
    }
    Ok(value)
}

pub(crate) fn validate_disk_thresholds(warning: u32, critical: u32) -> Result<(), String> {
    if !(MIN_DISK_FREE_GB..=MAX_DISK_FREE_GB).contains(&warning)
        || !(MIN_DISK_FREE_GB..=MAX_DISK_FREE_GB).contains(&critical)
        || critical >= warning
    {
        Err("Disk thresholds must be between 1 and 10000 GB, with critical below warning.".into())
    } else {
        Ok(())
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
        Ok(bytes) => serde_json::from_slice::<BusSettings>(&bytes)
            .map_err(|error| {
                format!(
                    "Bus settings at {} are unreadable; using defaults: {error}",
                    path.display()
                )
            })
            .and_then(|settings| {
                validate_disk_thresholds(settings.disk_warning_gb, settings.disk_critical_gb)?;
                Ok(settings)
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
    validate_idle_agents_alert(settings.idle_agents_alert)?;
    validate_disk_thresholds(settings.disk_warning_gb, settings.disk_critical_gb)?;
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
    fn feishu_notifications_default_on_with_recipient_setting() {
        let value = serde_json::to_value(BusSettings::default()).unwrap();
        assert_eq!(value["feishu_waiting_notifications"], true);
        assert_eq!(
            value["feishu_recipient"],
            "ou_ca1b530d792419a044e281f998ce68dc"
        );
        assert!(value.get("feishu_user_id").is_none());
        assert!(value.get("feishu_chat_id").is_none());
    }

    #[test]
    fn legacy_freeze_preference_is_dropped_without_changing_developer_alerts_or_sound() {
        let root = crate::utils::test_temp::unique_temp_path("bus-retired-freeze-setting");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("settings.json");
        for old_value in [false, true] {
            let old = serde_json::json!({
                "freeze_notifications": old_value,
                "feishu_waiting_notifications": false,
                "feishu_recipient": "ou_saved",
                "master_sound": {"enabled": false, "name": "Glass"},
                "sidebar_width": 35
            });
            std::fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
            let loaded = load(&path).unwrap();
            assert!(loaded.feishu_notifications);
            assert_eq!(loaded.feishu_recipient, "ou_saved");
            assert!(!loaded.master_sound.enabled);
            assert_eq!(loaded.master_sound.name.as_deref(), Some("Glass"));
            assert_eq!(loaded.sidebar_width, 35);
            save(&path, &loaded).unwrap();
            let saved: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert!(saved.get("freeze_notifications").is_none());
            assert_eq!(load(&path).unwrap(), loaded);
        }
        std::fs::remove_dir_all(root).unwrap();
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
    fn idle_agents_alert_defaults_to_five_and_validates_config_values() {
        let root = crate::utils::test_temp::unique_temp_path("bus-idle-agents-settings");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("settings.json");
        assert_eq!(load(&path).unwrap().idle_agents_alert, 5);
        for value in [1, 7, 10] {
            std::fs::write(&path, format!("{{\"idle_agents_alert\":{value}}}")).unwrap();
            let loaded = load(&path).unwrap();
            assert_eq!(loaded.idle_agents_alert, value);
            save(&path, &loaded).unwrap();
            assert_eq!(load(&path).unwrap(), loaded);
        }
        for value in ["0", "11", "-1", "1.5", "\"seven\""] {
            std::fs::write(&path, format!("{{\"idle_agents_alert\":{value}}}")).unwrap();
            assert!(load(&path).is_err(), "invalid limit {value}");
        }
        let invalid = BusSettings {
            idle_agents_alert: 11,
            ..BusSettings::default()
        };
        assert!(save(&path, &invalid).is_err());
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
