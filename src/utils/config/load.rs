use std::path::{Path, PathBuf};

use tracing::warn;

use crate::utils::env::bus_data_dir;

use super::{model::LoadedConfig, Config, CONFIG_PATH_ENV_VAR};

const KNOWN_TOP_LEVEL_CONFIG_KEYS: &[&str] = &[
    "advanced",
    "experimental",
    "onboarding",
    "server",
    "session",
    "terminal",
    "theme",
    "ui",
];

pub fn app_dir_name() -> &'static str {
    if cfg!(debug_assertions) {
        "herdr-dev"
    } else {
        "herdr"
    }
}

pub fn config_dir() -> PathBuf {
    if let Some(root) = bus_data_dir() {
        return root.join("herdr-config");
    }
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(dir).join(app_dir_name());
    }
    platform_config_dir()
}

pub fn state_dir() -> PathBuf {
    if let Some(root) = bus_data_dir() {
        return root.join("herdr-state");
    }
    if let Ok(dir) = std::env::var("XDG_STATE_HOME") {
        return PathBuf::from(dir).join(app_dir_name());
    }
    platform_state_dir()
}

#[cfg(windows)]
fn platform_config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("APPDATA") {
        return PathBuf::from(dir).join(app_dir_name());
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        return PathBuf::from(profile)
            .join("AppData")
            .join("Roaming")
            .join(app_dir_name());
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(format!(".config/{}", app_dir_name()));
    }
    std::env::temp_dir().join(app_dir_name())
}

#[cfg(not(windows))]
fn platform_config_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(format!(".config/{}", app_dir_name()))
    } else {
        std::env::temp_dir().join(app_dir_name())
    }
}

#[cfg(windows)]
fn platform_state_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("LOCALAPPDATA") {
        return PathBuf::from(dir).join(app_dir_name());
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        return PathBuf::from(profile)
            .join("AppData")
            .join("Local")
            .join(app_dir_name());
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(format!(".local/state/{}", app_dir_name()));
    }
    std::env::temp_dir().join(format!("{}-state", app_dir_name()))
}

#[cfg(not(windows))]
fn platform_state_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(format!(".local/state/{}", app_dir_name()))
    } else {
        std::env::temp_dir().join(format!("{}-state", app_dir_name()))
    }
}

fn read_optional_config(path: &Path) -> std::io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

impl Config {
    pub fn load() -> LoadedConfig {
        Self::load_inner()
    }

    fn load_inner() -> LoadedConfig {
        let path = config_path();
        let content = match read_optional_config(&path) {
            Ok(Some(content)) => content,
            Ok(None) => {
                return LoadedConfig {
                    config: Self::default(),
                    diagnostics: Vec::new(),
                    invalid_sections: Vec::new(),
                };
            }
            Err(err) => {
                warn!(err = %err, "config read error, using defaults");
                return LoadedConfig {
                    config: Self::default(),
                    diagnostics: vec![format!("config read error: {err}; using defaults")],
                    invalid_sections: Vec::new(),
                };
            }
        };

        match deserialize_with_ignored::<Config, _>(toml::Deserializer::new(&content)) {
            Ok((config, ignored_keys)) => {
                let (unknown_sections, mut unknown) = unknown_top_level_sections_from_str(&content);
                unknown.extend(unknown_config_key_diagnostics(
                    ignored_keys
                        .into_iter()
                        .filter(|path| {
                            !matches!(path.as_slice(), [ConfigKeyPathSegment::Key(key)] if unknown_sections.contains(key))
                        })
                        .collect(),
                    None,
                ));
                if !unknown.is_empty() {
                    warn!(unknown = ?unknown, "config has unknown keys, using defaults");
                    return LoadedConfig {
                        config: Self::default(),
                        diagnostics: unknown
                            .into_iter()
                            .map(|diagnostic| format!("{diagnostic}; using defaults"))
                            .collect(),
                        invalid_sections: Vec::new(),
                    };
                }
                LoadedConfig {
                    diagnostics: config.collect_diagnostics(),
                    config,
                    invalid_sections: Vec::new(),
                }
            }
            Err(err) => {
                warn!(err = %err, "config parse error, using defaults");
                LoadedConfig {
                    config: Self::default(),
                    diagnostics: vec![format!("config parse error: {err}; using defaults")],
                    invalid_sections: Vec::new(),
                }
            }
        }
    }
}

pub(super) fn resolve_config_relative_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }

    config_path()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(path)
}

pub fn config_path() -> PathBuf {
    if bus_data_dir().is_some() {
        return config_dir().join("config.toml");
    }
    if let Ok(path) = std::env::var(CONFIG_PATH_ENV_VAR) {
        return PathBuf::from(path);
    }
    config_dir().join("config.toml")
}

pub fn config_diagnostic_summary(diagnostics: &[String]) -> Option<String> {
    if diagnostics.is_empty() {
        return None;
    }

    let target = config_path()
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.toml")
        .to_string();
    let read_error = diagnostics
        .iter()
        .any(|diagnostic| diagnostic.starts_with("config read error:"));
    let impact = if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.contains("using defaults"))
    {
        if read_error {
            " unreadable; using defaults"
        } else {
            " invalid; using defaults"
        }
    } else if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.contains("keeping current config"))
    {
        if read_error {
            " unreadable; keeping current config"
        } else {
            " invalid; keeping current config"
        }
    } else {
        ""
    };

    Some(format!("{target}{impact}; herdr config check"))
}

pub fn load_live_config() -> Result<LoadedConfig, Vec<String>> {
    let path = config_path();
    let content = match read_optional_config(&path) {
        Ok(Some(content)) => content,
        Ok(None) => {
            return Ok(LoadedConfig {
                config: Config::default(),
                diagnostics: Vec::new(),
                invalid_sections: Vec::new(),
            });
        }
        Err(err) => {
            return Err(vec![format!(
                "config read error: {err}; keeping current config"
            )]);
        }
    };
    load_live_config_from_str(&content)
}

fn load_live_config_from_str(content: &str) -> Result<LoadedConfig, Vec<String>> {
    let value = content
        .parse::<toml::Value>()
        .map_err(|err| vec![format!("config parse error: {err}; keeping current config")])?;
    let table = value.as_table().ok_or_else(|| {
        vec![
            "config parse error: top-level config must be a table; keeping current config"
                .to_string(),
        ]
    })?;

    let mut config = Config::default();
    let mut unknown = unknown_top_level_section_diagnostics(table);
    unknown.extend(unknown_top_level_config_key_diagnostics(table));
    let mut diagnostics = Vec::new();
    let mut invalid_sections = Vec::new();

    if let Some(value) = table.get("onboarding") {
        match value.clone().try_into::<Option<bool>>() {
            Ok(onboarding) => config.onboarding = onboarding,
            Err(err) => diagnostics.push(format!(
                "invalid onboarding setting: {err}; keeping current onboarding state"
            )),
        }
    }

    load_live_section(
        table,
        "theme",
        "theme config",
        &mut unknown,
        &mut diagnostics,
        &mut invalid_sections,
        |section| config.theme = section,
    );
    load_live_section(
        table,
        "terminal",
        "terminal config",
        &mut unknown,
        &mut diagnostics,
        &mut invalid_sections,
        |section| config.terminal = section,
    );
    load_live_section(
        table,
        "session",
        "session config",
        &mut unknown,
        &mut diagnostics,
        &mut invalid_sections,
        |section| config.session = section,
    );
    load_live_section(
        table,
        "server",
        "server config",
        &mut unknown,
        &mut diagnostics,
        &mut invalid_sections,
        |section| config.server = section,
    );
    load_live_section(
        table,
        "ui",
        "ui config",
        &mut unknown,
        &mut diagnostics,
        &mut invalid_sections,
        |section| config.ui = section,
    );
    load_live_section(
        table,
        "advanced",
        "advanced config",
        &mut unknown,
        &mut diagnostics,
        &mut invalid_sections,
        |section| config.advanced = section,
    );
    load_live_section(
        table,
        "experimental",
        "experimental config",
        &mut unknown,
        &mut diagnostics,
        &mut invalid_sections,
        |section| config.experimental = section,
    );

    // An unknown key is a config error: the whole reload is rejected and the running config stays.
    if !unknown.is_empty() {
        return Err(unknown
            .into_iter()
            .map(|diagnostic| format!("{diagnostic}; keeping current config"))
            .chain(diagnostics)
            .collect());
    }

    diagnostics.extend(config.theme.diagnostics());

    Ok(LoadedConfig {
        config,
        diagnostics,
        invalid_sections,
    })
}

fn unknown_top_level_sections_from_str(content: &str) -> (Vec<String>, Vec<String>) {
    let Ok(value) = content.parse::<toml::Value>() else {
        return (Vec::new(), Vec::new());
    };
    let Some(table) = value.as_table() else {
        return (Vec::new(), Vec::new());
    };

    let mut keys = Vec::new();
    let mut diagnostics = Vec::new();
    for (key, value) in table {
        if let Some(diagnostic) = unknown_top_level_section_diagnostic(key, value) {
            keys.push(key.clone());
            diagnostics.push(diagnostic);
        }
    }
    (keys, diagnostics)
}

fn unknown_top_level_section_diagnostics(
    table: &toml::map::Map<String, toml::Value>,
) -> Vec<String> {
    table
        .iter()
        .filter_map(|(key, value)| unknown_top_level_section_diagnostic(key, value))
        .collect()
}

fn unknown_top_level_section_diagnostic(key: &str, value: &toml::Value) -> Option<String> {
    if KNOWN_TOP_LEVEL_CONFIG_KEYS.contains(&key) {
        return None;
    }

    let header = if value.is_table() {
        format!("[{key}]")
    } else if value
        .as_array()
        .is_some_and(|items| !items.is_empty() && items.iter().all(toml::Value::is_table))
    {
        format!("[[{key}]]")
    } else {
        return None;
    };

    if key == "toast" {
        Some(format!(
            "unknown config section {header} (did you mean [ui.toast]?)"
        ))
    } else {
        Some(format!("unknown config section {header}"))
    }
}

fn unknown_top_level_config_key_diagnostics(
    table: &toml::map::Map<String, toml::Value>,
) -> Vec<String> {
    let paths = table
        .iter()
        .filter(|(key, value)| {
            !KNOWN_TOP_LEVEL_CONFIG_KEYS.contains(&key.as_str())
                && unknown_top_level_section_diagnostic(key, value).is_none()
        })
        .map(|(key, _)| vec![ConfigKeyPathSegment::Key(key.clone())])
        .collect();
    unknown_config_key_diagnostics(paths, None)
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum ConfigKeyPathSegment {
    Key(String),
    Index(usize),
}

fn config_key_path(path: &serde_ignored::Path<'_>) -> Vec<ConfigKeyPathSegment> {
    fn visit(path: &serde_ignored::Path<'_>, segments: &mut Vec<ConfigKeyPathSegment>) {
        match path {
            serde_ignored::Path::Root => {}
            serde_ignored::Path::Seq { parent, index } => {
                visit(parent, segments);
                segments.push(ConfigKeyPathSegment::Index(*index));
            }
            serde_ignored::Path::Map { parent, key } => {
                visit(parent, segments);
                segments.push(ConfigKeyPathSegment::Key(key.clone()));
            }
            serde_ignored::Path::Some { parent }
            | serde_ignored::Path::NewtypeStruct { parent }
            | serde_ignored::Path::NewtypeVariant { parent } => visit(parent, segments),
        }
    }

    let mut segments = Vec::new();
    visit(path, &mut segments);
    segments
}

fn format_config_key_path(path: &[ConfigKeyPathSegment]) -> String {
    path.iter()
        .map(|segment| match segment {
            ConfigKeyPathSegment::Key(key)
                if !key.is_empty()
                    && key.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
                    }) =>
            {
                key.clone()
            }
            ConfigKeyPathSegment::Key(key) => toml::Value::String(key.clone()).to_string(),
            ConfigKeyPathSegment::Index(index) => index.to_string(),
        })
        .collect::<Vec<_>>()
        .join(".")
}

fn unknown_config_key_diagnostics(
    paths: Vec<Vec<ConfigKeyPathSegment>>,
    section: Option<&str>,
) -> Vec<String> {
    let mut paths: Vec<Vec<ConfigKeyPathSegment>> = paths
        .into_iter()
        .map(|mut path| {
            if let Some(section) = section {
                path.insert(0, ConfigKeyPathSegment::Key(section.to_string()));
            }
            path
        })
        .collect();
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .map(|path| format!("unknown config key {}", format_config_key_path(&path)))
        .collect()
}

fn deserialize_with_ignored<'de, T, D>(
    deserializer: D,
) -> Result<(T, Vec<Vec<ConfigKeyPathSegment>>), D::Error>
where
    T: serde::Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    let mut ignored = Vec::new();
    let value = serde_ignored::deserialize(deserializer, |path| {
        ignored.push(config_key_path(&path));
    })?;
    Ok((value, ignored))
}

fn load_live_section<T>(
    table: &toml::map::Map<String, toml::Value>,
    section: &'static str,
    label: &str,
    unknown: &mut Vec<String>,
    diagnostics: &mut Vec<String>,
    invalid_sections: &mut Vec<String>,
    apply: impl FnOnce(T),
) where
    T: serde::de::DeserializeOwned,
{
    let Some(value) = table.get(section) else {
        return;
    };

    match deserialize_with_ignored(value.clone()) {
        Ok((section_config, ignored_keys)) => {
            unknown.extend(unknown_config_key_diagnostics(ignored_keys, Some(section)));
            apply(section_config);
        }
        Err(err) => {
            diagnostics.push(format!(
                "invalid {label}: {err}; keeping current {section} settings"
            ));
            invalid_sections.push(section.to_string());
        }
    }
}

#[cfg(test)]
#[path = "tests/config_test.rs"]
mod tests;
