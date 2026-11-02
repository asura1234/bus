# Diagnose terminal driver input

Use `bus agent observe AGENT` for the entire current terminal screen. Use
`--cells` when cursor position, styles or focus need inspection. The observation
includes the terminal and pane IDs and content revision; no provider-specific
dialog parser or fingerprint is involved.

Answer with `bus agent press AGENT --select 'complete visible option label'`,
`--key KEY`, or `--text 'answer' --submit`. `--text` alone types without Enter.
Bind input to the observed terminal with `--terminal-id TERMINAL_ID`. For shell
questions and pagers, add `--pane` and use the pane or terminal ID as the target.

The native writer checks the completed observation while holding the same content
lock as PTY updates and keeps that lock through enqueue. A changed revision or an
unfinished synchronized frame returns `terminal_observation_changed` without
writing any bytes. Observe the screen again before deciding what to send. A read
can also return `terminal_observation_unavailable` while the terminal redraws.

Selection sends one navigation key, reads the redraw, and checks focus before
continuing. It confirms with Enter only after the requested label is focused.
Ambiguous or missing labels/focus require another observation or explicit keys.
`selection_pending` or `unknown` preserves the keys already sent and the latest
observation: inspect that result before continuing. Never blindly repeat an
uncertain action.

With dev logging, correlate `bus.terminal.driver.result` by API request ID and
terminal ID. It records outcome and input metadata, including keys and whether
text was written. A successful queue receipt proves enqueueing; inspect the
resulting screen and, for a Bus task, its durable message status to prove what
the terminal program did.

The captures under `tests/fixtures/dialog-fingerprint/` are historical terminal
screens from the retired parser. They are not evidence that a current driver
run succeeded. Keep fresh acceptance artifacts bound to the exact compiled SHA,
terminal identities and native driver route records.
