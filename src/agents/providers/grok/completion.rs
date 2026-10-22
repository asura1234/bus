//! Lane B resolves a stop candidate after committed completion evidence.
use serde_json::Value;
#[derive(Debug, PartialEq)]
#[allow(dead_code)] // G0 freezes variants before lane B produces them.
pub(crate) enum CompletionResolution {
    Pending,
    Final {
        session: String,
        turn: String,
        text: String,
    },
    Cancelled {
        session: String,
        turn: String,
        reason: String,
    },
}
pub(crate) fn resolve_candidate(
    _value: &Value,
    _settled_idle: bool,
) -> Result<CompletionResolution, String> {
    Err("Grok completion resolution is not implemented (G0; lane B)".into())
}
