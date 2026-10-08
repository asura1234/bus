use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    /// Resume supported AI-agent panes into their native conversation sessions
    /// when restoring a Herdr session. Default: true.
    pub resume_agents_on_restore: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            resume_agents_on_restore: true,
        }
    }
}
