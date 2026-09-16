//! User preferences shared by every local Bus session.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct BusSettings {
    pub(crate) color_blind_mode: bool,
    pub(crate) orchestrator: OrchestratorSettings,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct OrchestratorSettings {
    pub(crate) enabled: bool,
    pub(crate) model: OrchestratorModelSetting,
    pub(crate) content_selector: OrchestratorContentSetting,
    pub(crate) system_prompt_override: Option<String>,
}

impl Default for OrchestratorSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            model: OrchestratorModelSetting::DeepSeekV41Flash,
            content_selector: OrchestratorContentSetting::Production,
            system_prompt_override: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum OrchestratorModelSetting {
    #[default]
    DeepSeekV41Flash,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum OrchestratorContentSetting {
    TestAgentLed,
    #[default]
    Production,
}

/// Registry sessions share one file beside the registry; an explicit
/// `BUS_DATA_DIR` root stays isolated with its own copy.
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
    fn orchestrator_system_prompt_override_round_trips_multiline_text() {
        let root = std::env::temp_dir().join(format!(
            "bus-settings-prompt-{}-{}",
            std::process::id(),
            super::super::io::now_ns()
        ));
        let path = root.join("settings.json");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            &path,
            br#"{"orchestrator":{"system_prompt_override":"Custom conductor\nUse repo-native skills."}}"#,
        )
        .unwrap();

        let loaded = load(&path).unwrap();
        let encoded = serde_json::to_value(loaded).unwrap();
        assert_eq!(
            encoded["orchestrator"]["system_prompt_override"],
            "Custom conductor\nUse repo-native skills."
        );

        let _ = std::fs::remove_dir_all(root);
    }
}
