//! Pure terminal identity predicates used during agent deletion.
use super::{AgentRuntimeIdentity, RoomAgent};

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

/// Bus already closed this exact terminal after saving the exited provider's screen.
pub(super) fn terminal_already_closed(agent: &RoomAgent, terminal: &str) -> bool {
    agent.exit_snapshot.as_ref().is_some_and(|snapshot| {
        snapshot.terminal_closed == Some(true) && snapshot.terminal_id == terminal
    })
}
