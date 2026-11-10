//! How long Bus waits before a stalled request needs recovery.

/// How long a queued message may wait on an agent that is not working before
/// it needs recovery. Delivery normally starts within a status tick of the
/// agent going idle, so two minutes means something refuses it.
pub(crate) const QUEUED_STALL_MS: u64 = 2 * 60 * 1000;
/// Existing Kimi delivered-turn retention window. Its late-start handling
/// keeps this bound; other idle submissions use the short confirmation bound.
pub(crate) const DELIVERED_STALL_MS: u64 = 3 * 60 * 1000;
/// An idle provider that has not confirmed Enter must release its inbox promptly.
/// Working, dialogs and unfinished native I/O still defer recovery. Kimi retains
/// its existing delivered-turn bound until that integration is revisited.
pub(crate) const UNCONFIRMED_SUBMISSION_MS: u64 = 5_000;
/// How long an agent may stay blocked (a dialog, question or trust prompt)
/// before `message status` reports `blocked_unanswered`. Answering takes a
/// person or the orchestrator a moment; five minutes untouched means nobody
/// saw it.
pub(crate) const BLOCKED_STALL_MS: u64 = 5 * 60 * 1000;

/// Preserve the deferred integration's existing bound while ordinary idle
/// submissions recover promptly when their Enter has no confirmed start.
pub(super) fn submission_confirmation_grace(provider: super::Provider) -> u64 {
    if provider == super::Provider::Kimi {
        DELIVERED_STALL_MS
    } else {
        UNCONFIRMED_SUBMISSION_MS
    }
}
