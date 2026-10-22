//! Provider harness contracts, independent of rooms and terminal ownership.
//!
//! Dispatch uses explicit matches between the concrete harnesses.

pub(crate) mod callback_entry;
pub(crate) mod claude_code;
pub(crate) mod codex;
pub(crate) mod cursor;
pub(crate) mod grok;
pub(crate) mod hook_json;
pub(crate) mod launch;
pub(crate) mod spool;
pub(crate) mod suggest;
pub(crate) mod usage;

use serde::{Deserialize, Serialize};

/// The provider harnesses Bus launches and observes. This is distinct
/// from the larger catalog of agents detected in a terminal.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProviderKind {
    Codex,
    ClaudeCode,
    Cursor,
    Grok,
}

impl ProviderKind {
    /// Canonical CLI/session label. Claude's persisted spelling remains
    /// `claude_code`; its CLI/session label has always been `claude`.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude",
            Self::Cursor => "cursor",
            Self::Grok => "grok",
        }
    }

    /// Executable used by the existing provider launch availability check.
    pub(crate) const fn executable(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude",
            Self::Cursor => "cursor-agent",
            Self::Grok => "grok",
        }
    }
}

pub(crate) fn parse(
    provider: ProviderKind,
    value: &serde_json::Value,
) -> Result<spool::Parsed, String> {
    if provider == ProviderKind::Grok
        && value
            .get("hookEventName")
            .and_then(serde_json::Value::as_str)
            == Some("notification")
        && value
            .get("notificationType")
            .and_then(serde_json::Value::as_str)
            == Some("idle_prompt")
        && value
            .get("subagentType")
            .is_none_or(serde_json::Value::is_null)
    {
        return Ok(spool::Parsed::ReconcileSession {
            session: spool::field(value, "sessionId")?,
        });
    }
    match provider {
        ProviderKind::Codex => codex::hooks::parse(value),
        ProviderKind::ClaudeCode => claude_code::hooks::parse(value),
        ProviderKind::Cursor => cursor::hooks::parse(value),
        ProviderKind::Grok => grok::hooks::parse(value),
    }
}

#[cfg(test)]
#[path = "tests/resume_test.rs"]
mod resume_tests;

#[cfg(test)]
#[path = "tests/grok_registration_test.rs"]
mod grok_registration_tests;
