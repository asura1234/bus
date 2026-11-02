# Dialog attention and numbered native drafts

## Run contract

Use fresh scratch sessions through `bus --dev tui` at 120x40 with inherited
`BUS_*` and `HERDR_*` removed. Use fake providers only. No desktop automation,
live Bus/data/provider or SKIP is allowed. Capture/open cell screenshots for
visual assertions. State, history, agent reads and observe/press may corroborate.

## Setup

Create `dialog-lab`, then add each case's worker with
`bus agent add --room dialog-lab --name worker --provider PROVIDER --pwd WORK
--consent-hooks`, where WORK is the scratch work directory. Wait until idle and
select `room:dialog-lab`. Beyond setup, use the UI for native editing and message
submission. Do not edit stored state.

### DA-01 — Numbered Claude composer edits do not request help

Use a Claude worker.

1. Click `agent:worker`. Type `1. Confirm the bug`, press `ctrl+j`, then type
   `2. Add a regression test`, one action per call. Wait 1500ms for stable
   native rows. `bus agent observe worker` shows the numbered draft between
   the composer rules; state is not blocked and history has zero Blocked and
   zero Resolved notices. Capture/open a cell screenshot.
2. Press `ctrl+j`, type `3. Run the gates`, then append ` with coverage`, one
   action per call. After stable rows, observation still shows an editable
   draft, with no Blocked/Resolved notice. Capture/open a cell screenshot.
3. Press `ctrl+u` five times. After stable rows, the draft is empty, the worker
   is not blocked, and notice counts remain zero. Stop the session.

### DA-02 — Each real dialog episode reports blocked once

Use a Codex worker. `fake:dialog-null` holds an inert permission menu until
Enter, then draws a 350ms synchronized redraw. It executes nothing.

1. Click `room:dialog-lab`, press `ctrl+p`, click `recipient:worker`, press
   `esc`, click `composer`, type `fake:dialog-null`, then press `enter`, one
   action per call. Wait for Blocked. Save history and `bus agent observe worker`:
   exactly one Blocked notice, zero Resolved notices and the focused native menu.
   After stable chat rows for 1500ms, those counts remain unchanged. Capture/open
   a cell screenshot.
2. Observe again and run `bus agent press worker --select 'Yes, proceed (y)'
   --terminal-id TERMINAL_ID`. Require outcome `selected`; wait for a completed
   redraw, idle state and the fake reply. Observation shows the ordinary composer.
   The episode retires quietly: exactly one Blocked and zero Resolved notices.
   Recheck after stable chat rows for 1500ms.
3. Submit `fake:dialog-null` again. Wait for a second Blocked notice, observe
   and select the same label through `agent press`, bound to the fresh terminal
   observation. Prove idle and a second fake reply. History has exactly two
   Blocked and zero Resolved notices. Capture/open a screenshot; stop the session.

### DA-03 — Numbered Codex composer edits do not request help

Use a Codex worker.

1. Click `agent:worker`. Type `1. Confirm the bug`, press `ctrl+j`, then type
   `2. Add a regression test`, one action per call. After stable rows for
   1500ms, `bus agent observe worker` shows the numbered draft above the model
   footer, state is not blocked and history has zero Blocked/Resolved notices.
   Capture/open a cell screenshot.
2. Press `ctrl+j`, type `3. Run the gates`, then append ` with coverage`, one
   action per call. After stable rows, the draft stays editable and notice counts
   remain zero. Capture/open a cell screenshot.
3. Press `ctrl+u` five times. After stable rows the draft is empty, state is
   not blocked and notice counts remain zero. Stop the session.
