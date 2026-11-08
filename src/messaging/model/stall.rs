//! How long Bus waits before a stalled request needs recovery.

/// How long a queued message may wait on an agent that is not working before
/// it needs recovery. Delivery normally starts within a status tick of the
/// agent going idle, so two minutes means something refuses it.
pub(crate) const QUEUED_STALL_MS: u64 = 2 * 60 * 1000;
/// How long a typed message may sit with its agent idle and no hook progress.
/// Longer than the queued grace: providers can take a while to report the
/// turn start or the final reply after the screen goes idle.
pub(crate) const DELIVERED_STALL_MS: u64 = 3 * 60 * 1000;
/// How long an agent may stay blocked (a dialog, question or trust prompt)
/// before `message status` reports `blocked_unanswered`. Answering takes a
/// person or the orchestrator a moment; five minutes untouched means nobody
/// saw it.
pub(crate) const BLOCKED_STALL_MS: u64 = 5 * 60 * 1000;
