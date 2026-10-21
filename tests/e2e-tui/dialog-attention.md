# Dialog attention and numbered native drafts

## Run contract

Use fresh scratch sessions through `bus --dev tui` at 120x40 with inherited
`BUS_*` and `HERDR_*` removed. Use fake providers only. No desktop automation,
real screenshots, live Bus, or live data is allowed. No SKIP is allowed.
Capture cell screenshots and open them for visual assertions. Read-only scratch
state, history, agent reads and dialog APIs may corroborate each step.

## Setup

The session status supplies its private work directory WORK. CLI fixture setup
is allowed: create `dialog-lab`, then add the case's worker with
`bus agent add --room dialog-lab --name worker --provider PROVIDER --pwd WORK
--consent-hooks`. Wait for the worker to be idle. Beyond setup, use the UI for
native editing and message submission. CLI is limited to read-only observations
and the dialog/choose API tested below. Do not edit stored state.

### DA-01 — Numbered native composer edits do not request help

Use a Claude worker.

1. Complete setup. Click `agent:worker` to open its native terminal. Type
   `1. Confirm the bug`, press `ctrl+j`, then type `2. Add a regression test`,
   one call per action. Wait for the native composer rows to be stable for
   1500ms. Read `bus agent dialog worker`, state and room history. The native
   numbered draft is visible between Claude's composer rules, dialog is null,
   and history has zero Blocked and zero Resolved notices. Capture and open a
   cell screenshot.
2. Press `ctrl+j` and type `3. Run the gates`, then append ` with coverage`,
   one call per action. Wait for the native composer rows to be stable for
   1500ms. Dialog remains null and history still has zero Blocked/Resolved
   notices. Capture and open a cell screenshot.
3. Press `ctrl+u` five times to clear the three-line native draft. Wait for the
   composer rows to be stable for 1500ms. The draft is empty, dialog is null,
   and history still has zero Blocked/Resolved notices. Stop the session.

### DA-02 — Each real dialog episode reports blocked and resolved once

Use a Codex worker. The inert `fake:dialog-null` fixture holds its permission
menu until Enter and draws a 350ms synchronized redraw after the answer. It
never executes the displayed command.

1. Complete setup. Click `room:dialog-lab`, press `ctrl+p`, click
   `recipient:worker`, press `esc`, click `composer`, type `fake:dialog-null`,
   and press `enter`, one call per action. Wait for the Blocked notice. Read
   room history and `bus agent dialog worker`: exactly one Blocked notice,
   zero Resolved notices, and one readable choice dialog with fingerprint.
   Wait for the chat rows to be stable for 1500ms and read history again;
   those counts stay unchanged. Capture and open a cell screenshot.
2. Choose option 1 with the observed fingerprint using `bus agent choose`.
   It reports `closed` after the synchronized redraw. Wait for the worker to
   be idle and for the Resolved notice. A fresh dialog read is null. History
   contains exactly one Blocked, one Resolved, and the fake reply. Wait for
   stable chat rows for 1500ms and recheck unchanged counts.
3. Submit `fake:dialog-null` again through the same composer. Wait for the
   second Blocked notice, then observe and choose option 1 with its new
   fingerprint. Wait for idle and the second Resolved notice. History contains
   exactly two Blocked and two Resolved notices. Capture and open a cell
   screenshot. Stop the session.
