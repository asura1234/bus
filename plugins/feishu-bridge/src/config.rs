use crate::error::{invalid, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub app_id: String,
    pub lark_cli: PathBuf,
    pub state_dir: PathBuf,
    pub allowed_sender_open_ids: Vec<String>,
    pub allowed_chat_ids: Vec<String>,
    pub sink_command: Vec<String>,
    pub command_timeout_seconds: u64,
    pub sink_timeout_seconds: u64,
    pub retry_seconds: u64,
    pub reconcile_seconds: u64,
    pub overlap_seconds: u64,
    pub initial_history_start: String,
}
pub fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or_else(|| invalid("HOME must be absolute"))
}
pub fn config_root() -> Result<PathBuf> {
    match std::env::var_os("XDG_CONFIG_HOME") {
        Some(p) if Path::new(&p).is_absolute() => Ok(PathBuf::from(p)),
        _ => Ok(home()?.join("Library/Application Support")),
    }
}
pub fn config_path() -> Result<PathBuf> {
    Ok(config_root()?.join("feishu-bridge/config.toml"))
}
impl Default for Config {
    fn default() -> Self {
        Self {
            app_id: String::new(),
            lark_cli: "/opt/homebrew/bin/lark-cli".into(),
            state_dir: home()
                .unwrap_or_default()
                .join("Library/Application Support/feishu-bridge/state"),
            allowed_sender_open_ids: vec![],
            allowed_chat_ids: vec![],
            sink_command: vec![],
            command_timeout_seconds: 30,
            sink_timeout_seconds: 30,
            retry_seconds: 5,
            reconcile_seconds: 60,
            overlap_seconds: 300,
            initial_history_start: "0".into(),
        }
    }
}
pub fn valid_id(s: &str, prefix: &str) -> bool {
    s.strip_prefix(prefix).is_some_and(|tail| {
        !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    })
}
impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            return Err(invalid("config path must be absolute"));
        }
        let c: Self = toml::from_str(&std::fs::read_to_string(path)?)
            .map_err(|_| invalid("invalid TOML configuration"))?;
        if !valid_id(&c.app_id, "cli_") || !c.lark_cli.is_absolute() || !c.state_dir.is_absolute() {
            return Err(invalid(
                "app_id and absolute lark_cli/state_dir are required",
            ));
        }
        for d in [
            c.command_timeout_seconds,
            c.sink_timeout_seconds,
            c.retry_seconds,
            c.reconcile_seconds,
            c.overlap_seconds,
        ] {
            if !(1..=86400).contains(&d) {
                return Err(invalid("durations must be 1..86400 seconds"));
            }
        }
        if c.initial_history_start
            .parse::<i64>()
            .ok()
            .filter(|v| *v >= 0)
            .is_none()
        {
            return Err(invalid("invalid initial_history_start"));
        }
        if c.allowed_sender_open_ids
            .iter()
            .any(|v| !valid_id(v, "ou_"))
            || c.allowed_chat_ids.iter().any(|v| !valid_id(v, "oc_"))
        {
            return Err(invalid("allowlists must contain identity IDs"));
        }
        Ok(c)
    }
    pub fn listener(&self) -> Result<()> {
        if self.allowed_sender_open_ids.is_empty()
            || self.allowed_chat_ids.is_empty()
            || self.sink_command.is_empty()
            || !Path::new(&self.sink_command[0]).is_absolute()
        {
            return Err(invalid(
                "listen needs sender/chat allowlists and an absolute sink executable",
            ));
        }
        Ok(())
    }
    pub fn permits(&self, v: &serde_json::Value) -> bool {
        v["app_id"] == self.app_id
            && v["chat_type"] == "p2p"
            && v["sender"]["sender_type"] == "user"
            && v["chat_id"]
                .as_str()
                .is_some_and(|s| self.allowed_chat_ids.iter().any(|c| c == s))
            && v["sender"]["open_id"]
                .as_str()
                .is_some_and(|s| self.allowed_sender_open_ids.iter().any(|c| c == s))
    }
}
