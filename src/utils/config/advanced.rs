use super::DEFAULT_SCROLLBACK_LIMIT_BYTES;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct AdvancedConfig {
    /// Maximum scrollback buffer size in bytes retained per pane terminal. Default: 10000000.
    pub scrollback_limit_bytes: usize,
}

impl Default for AdvancedConfig {
    fn default() -> Self {
        Self {
            scrollback_limit_bytes: DEFAULT_SCROLLBACK_LIMIT_BYTES,
        }
    }
}
