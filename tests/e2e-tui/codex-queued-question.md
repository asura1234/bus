# Codex queued question

## Run contract

Use the e2e-test skill and a verified dev-tools binary built under the active
room's slot and disk policy. Run CQ-01 in a fresh scratch session at **160x45**,
default kitty graphics. No SKIP is allowed. Remove inherited `BUS_*` and
`HERDR_*` variables from each dev TUI driver invocation. Never drive the live
Bus or run a real provider.

The fake Codex `fake:question` prompt reproduces the collapsed question beside
an empty composer and a footer warning. It displays `? 1 question` and
`⇧← to answer`. Shift+Left opens `Which design should the worker use?` with
Alpha, Beta and Other. Enter on Alpha or Beta answers the inert fixture.

## Fixture setup

`T` means the verified binary's `--dev tui` with the case's `--name` on every
call. Start the session and record `status`. WORK is its scratch work directory.

```text
T bus room create question-lab
T bus agent add --room question-lab --name worker --provider codex --pwd WORK --consent-hooks
```

Wait until the worker is idle. Beyond setup, use the UI for message submission;
CLI state/history/agent reads and the observe/press driver may corroborate it.

### CQ-01 — A collapsed question beside a footer warning is readable and answerable

1. Click `room:question-lab`, press `ctrl+p`, click `recipient:worker`, click
   `composer`, paste `fake:question`, then press `enter`, one action per driver
   call. Wait for the worker's Blocked notice with `Inspect: bus agent observe N`.
   Save state, the notice and `T bus agent observe worker`. The full screen must
   contain `? 1 question`, `⇧← to answer` and `⚠ 1 warning · f2 to view`.
2. Read `T bus agent read worker --source recent --lines 120` while it is busy.
   It succeeds with `capture.exhausted: false`, a note that only the visible
   screen was read while busy, and text containing `? 1 question`.
3. Run `T bus agent observe worker` again. Inspection must leave the collapsed
   screen unchanged. Send the visible shortcut explicitly with
   `T bus agent press worker --key shift+left --terminal-id TERMINAL_ID`, using
   the observed terminal ID. Observe until the question and Alpha/Beta/Other
   rows are visible, then capture and open a cell screenshot.
4. Run `T bus agent press worker --select 'Beta' --terminal-id TERMINAL_ID`.
   Require outcome `selected` and no pending selection; that receipt proves
   enqueueing. Within 30s, a fresh observation must show the ordinary composer,
   the worker must return idle, and history must contain
   `fake reply: fake:question`. The closed episode adds no Resolved notice.
   Stop the scratch session.

A pending or uncertain input is evidence to inspect, never permission to replay
keys blindly. No parser, automatic panel opening or fingerprint is involved.
