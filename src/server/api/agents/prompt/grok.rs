//! Lane C supplies guarded Grok composer observation.
use crate::terminal::runtime::InputObservation;
pub(super) fn observe(_screen: &str, _expected: &str) -> InputObservation {
    InputObservation::Unavailable
}
pub(super) fn input(_screen: &str) -> Option<String> {
    None
}
