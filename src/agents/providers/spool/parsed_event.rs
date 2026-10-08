//! Provider observations, before launch/session/request correlation.
use serde_json::Value;

/// Provider parsers return facts only. Session and turn identifiers remain the
/// provider's opaque strings; no room identity or disposition belongs here.
#[derive(Debug, PartialEq)]
pub(crate) enum Parsed {
    /// `source` is the provider's start reason, e.g. `startup`, `resume` or `compact`.
    Session {
        session: String,
        source: Option<String>,
    },
    /// The provider accepted this prompt for the identified turn.
    Started {
        session: String,
        turn: String,
        prompt: String,
    },
    /// A final reply carried by the completion hook itself.
    Final {
        session: String,
        turn: String,
        text: String,
    },
    /// Background work will wake this turn again; this is not a final reply.
    BackgroundPending { session: String, turn: String },
    /// Response text observed separately from completion (Cursor today).
    /// The coordinator chooses its companion and resolves the settled reply.
    Response {
        session: String,
        turn: String,
        text: String,
    },
    /// Successful completion without a settled reply in this callback.
    /// This must not settle a request without the coordinator's reply resolution.
    Completed { session: String, turn: String },
    /// Provider failure; the coordinator retains existing request ownership.
    Failure {
        session: String,
        turn: String,
        message: String,
    },
    /// A valid hook that does not represent an interactive terminal observation.
    Ignore,
}

impl Parsed {
    /// Keep diagnostic values stable even where Rust variant names are neutral.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Session { .. } => "session",
            Self::Started { .. } => "started",
            Self::Final { .. } => "final",
            Self::BackgroundPending { .. } => "background_pending",
            Self::Response { .. } => "cursor_response",
            Self::Completed { .. } => "cursor_stop",
            Self::Failure { .. } => "failure",
            Self::Ignore => "ignored",
        }
    }
}

/// Shared required-string decoding with the existing hook error text.
/// Empty strings are invalid; whitespace and provider text remain verbatim.
pub(crate) fn field(value: &Value, key: &str) -> Result<String, String> {
    value.get(key).and_then(Value::as_str).filter(|v| !v.is_empty()).map(str::to_owned)
        .ok_or_else(|| format!("Hook missing {key}; update the CLI and verify Bus hooks. No prompt will be retried."))
}
