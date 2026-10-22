//! No verified subscription allowance source; session tokens are not quota.
pub(crate) fn allowance_unavailable_reason() -> &'static str {
    "Grok has no verified local allowance source"
}

#[cfg(test)]
#[path = "tests/usage_test.rs"]
mod usage_test;
