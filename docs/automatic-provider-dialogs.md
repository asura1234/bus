# Automatic provider UI answers

Bus has a closed allowlist of native provider dialogs whose answer is fixed.
The initial entries are Claude's and Codex's “Giving this request a little extra
thought” prompt: Bus chooses **Dismiss and keep waiting**, retaining the current
model. Claude's identity comes from retained native observations. The Codex
fixture records the exact parsed dialog supplied from a real Bus observation;
it is not a native PTY capture. An unrecognised Codex variant stays on the
ordinary dialog path.

Recognition uses the provider and the complete parsed dialog shape ID, covering
the title, kind, and every numbered option. A matching phrase in terminal text
or a question is insufficient. A fresh coherent native observation must still
show a single choice, no tabs, and no text field. Changed wording or options do
not match the allowlist. The recorded shape and provenance are in
`tests/fixtures/provider-ui/README.md`.

Command approvals, file writes, permissions, product questions, update choosers,
and all other shapes retain their existing routing. They are never added by a
text-pattern fallback.

Automatic answers use two guarded phases. Focus sends only navigation arrows.
After an ordinary poll sees the target selected, a fresh fingerprint permits
confirmation. The native confirm action refuses to navigate: it sends Enter
only when the requested option is already selected. Both phases use the shared
bounded synchronized-frame guard, so a repaint cannot lend a partial grid or a
replacement permission dialog to the earlier action. Ordinary manual choice
submission keeps its existing behavior.

Before keys are sent, Bus durably reserves the phase. Automatic attempts add no
room-history notice, orchestrator request or developer-attention wake. The
structured `bus.dialog.auto_answer` log records the allowlist rule, option,
phase and outcome without copying prompt text. Ordinary help notifications
remain available when automatic handling cannot finish.

An uncertain confirmation is not replayed, including after a restart. A new
launch or a coherently closed dialog allows a later occurrence of the same
provider prompt to start another episode. Storage failure prevents key input.
If a spent confirmation remains open or focus does not reach the target, the
ordinary blocked-dialog path asks the orchestrator exactly once. The reservation
prevents another Enter, and a later focus redraw cannot resume automation after
handoff, including after restart. These boundaries never authorize an answer to
a different dialog.
