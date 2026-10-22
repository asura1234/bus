//! Lane B supplies Grok event decoding without publishing stop-gate attempts.
use crate::agents::providers::{spool::Parsed, ProviderKind};
use serde_json::Value;
pub(crate) fn parse(_value: &Value) -> Result<Parsed, String> {
    Err("Grok hook decoding is not implemented (G0; lane B)".into())
}
pub(crate) fn is_foreign_callback(_adapter: ProviderKind, _value: &Value) -> bool {
    false
}
