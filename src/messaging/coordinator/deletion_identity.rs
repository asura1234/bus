//! Pure classification of incomplete identities during agent deletion.
use super::AgentRuntimeIdentity;

/// A known terminal's pane is detached only after authoritative
/// absence/replacement evidence or a successful guarded close.
pub(super) fn has_no_terminal_to_close(identity: &AgentRuntimeIdentity) -> bool {
    identity == &AgentRuntimeIdentity::default()
        || identity.pane_id.is_none() && identity.terminal_id.is_some()
}

/// Tab creation can time out before AgentStart is issued. No provider
/// input was sent without a returned terminal identity. Remove only
/// the placeholder after a fresh native lookup proves it owns nothing;
/// never guess which shell a lost tab-create response may have made.
pub(super) fn needs_native_absence_check(identity: &AgentRuntimeIdentity) -> bool {
    identity.pane_id.is_none() && identity.terminal_id.is_none() && identity.session_id.is_none()
}
