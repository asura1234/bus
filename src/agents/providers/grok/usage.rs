//! Lane A+D owns Grok's explicit absence of a v1 allowance source.
pub(crate) fn allowance_unavailable_reason() -> &'static str {
    "Grok has no verified local allowance source"
}
