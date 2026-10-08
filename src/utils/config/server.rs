use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Virtual terminal width used when no client is attached. Default: 120.
    pub headless_cols: u16,
    /// Virtual terminal height used when no client is attached. Default: 40.
    pub headless_rows: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            headless_cols: crate::config::DEFAULT_HEADLESS_COLS,
            headless_rows: crate::config::DEFAULT_HEADLESS_ROWS,
        }
    }
}
