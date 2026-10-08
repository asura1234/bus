use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Current snapshot format version.
pub(super) const SNAPSHOT_VERSION: u32 = 3;

/// Serializable snapshot of the entire herdr session.
#[derive(Serialize, Deserialize)]
pub struct SessionSnapshot {
    /// Format version — used to detect incompatible changes.
    #[serde(default)]
    pub version: u32,
    pub workspaces: Vec<WorkspaceSnapshot>,
    pub active: Option<usize>,
    pub selected: usize,
}

#[derive(Serialize, Deserialize)]
pub struct SessionHistorySnapshot {
    /// Format version follows the matching session snapshot version.
    #[serde(default)]
    pub version: u32,
    pub workspaces: Vec<WorkspaceHistorySnapshot>,
}

#[derive(Serialize, Deserialize)]
pub struct WorkspaceHistorySnapshot {
    pub tabs: Vec<TabHistorySnapshot>,
}

#[derive(Serialize, Deserialize)]
pub struct TabHistorySnapshot {
    pub panes: HashMap<u32, PaneHistorySnapshot>,
}

#[derive(Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub custom_name: Option<String>,
    pub identity_cwd: PathBuf,
    #[serde(default)]
    pub public_pane_numbers: HashMap<u32, usize>,
    #[serde(default)]
    pub next_public_pane_number: usize,
    #[serde(default)]
    pub public_tab_numbers: Vec<usize>,
    #[serde(default)]
    pub next_public_tab_number: usize,
    pub tabs: Vec<TabSnapshot>,
    #[serde(default)]
    pub active_tab: usize,
}

#[derive(Serialize, Deserialize)]
pub struct TabSnapshot {
    #[serde(default)]
    pub custom_name: Option<String>,
    pub layout: LayoutSnapshot,
    pub panes: HashMap<u32, PaneSnapshot>,
    pub zoomed: bool,
    #[serde(default)]
    pub focused: Option<u32>,
    #[serde(default)]
    pub root_pane: Option<u32>,
}

#[derive(Serialize, Deserialize)]
pub struct PaneSnapshot {
    pub cwd: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_agent_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session: Option<PaneAgentSessionSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_argv: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize)]
pub struct PaneAgentSessionSnapshot {
    pub source: String,
    pub agent: String,
    pub kind: crate::agent_resume::AgentSessionRefKind,
    pub value: String,
}

#[derive(Serialize, Deserialize)]
pub struct PaneHistorySnapshot {
    pub ansi: String,
    pub lines: usize,
}

/// Serializable BSP tree.
#[derive(Serialize, Deserialize)]
pub enum LayoutSnapshot {
    Pane(u32),
    Split {
        direction: DirectionSnapshot,
        ratio: f32,
        first: Box<LayoutSnapshot>,
        second: Box<LayoutSnapshot>,
    },
}

#[derive(Serialize, Deserialize)]
pub enum DirectionSnapshot {
    Horizontal,
    Vertical,
}

#[cfg(test)]
pub(super) use super::capture::capture_node;
#[cfg(test)]
use super::capture::{capture, capture_history};
#[cfg(test)]
use crate::terminal::TerminalRuntimeRegistry;

pub(super) fn parse_snapshot(content: &str) -> Result<SessionSnapshot, String> {
    let snapshot = serde_json::from_str::<SessionSnapshot>(content).map_err(|e| e.to_string())?;
    if snapshot.version > SNAPSHOT_VERSION {
        return Err(format!(
            "snapshot version {} is newer than supported {}",
            snapshot.version, SNAPSHOT_VERSION
        ));
    }
    Ok(snapshot)
}

pub(super) fn parse_history_snapshot(content: &str) -> Result<SessionHistorySnapshot, String> {
    let snapshot =
        serde_json::from_str::<SessionHistorySnapshot>(content).map_err(|e| e.to_string())?;
    if snapshot.version > SNAPSHOT_VERSION {
        return Err(format!(
            "history snapshot version {} is newer than supported {}",
            snapshot.version, SNAPSHOT_VERSION
        ));
    }
    Ok(snapshot)
}

pub(super) fn snapshot_file_version(content: &str) -> Option<u32> {
    #[derive(Deserialize)]
    struct Version {
        #[serde(default)]
        version: u32,
    }
    serde_json::from_str::<Version>(content)
        .ok()
        .map(|file| file.version)
}

#[cfg(test)]
#[path = "schema_test.rs"]
mod tests;
