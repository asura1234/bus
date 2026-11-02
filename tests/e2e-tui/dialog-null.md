# Soft-wrapped Codex permission dialog

## Run contract

Use a fresh scratch Bus through the dev TUI driver at **80x35**, with inherited
`BUS_*` and `HERDR_*` removed and a verified dev-tools binary built under the
active room's slot policy. No live provider, live Bus or SKIP is allowed. The
inert `fake:dialog-null` fixture shows a captured Codex permission screen whose
second option wraps before an indented shell redirection. It executes nothing.

## Setup

Start the scratch session. Its `status` supplies WORK. CLI setup is allowed:

```text
T bus room create dialog-lab
T bus agent add --room dialog-lab --name worker --provider codex --pwd WORK --consent-hooks
```

Wait until idle. Beyond setup, use the UI for submission; state, history, visible
agent reads and observe/press are the allowed corroborating CLI surfaces.

### DN-01 — A soft-wrapped permission is readable and answerable

1. Click `room:dialog-lab`, press `ctrl+p`, click `recipient:worker`, click
   `composer`, paste `fake:dialog-null`, then press `enter`, one action per call.
   Wait for Blocked. Save state and `bus agent observe worker`; all three options,
   the second option's wrapped command and `> /tmp/dialog-probe.log 2>&1` must be
   readable. Its viewport is narrower than the 122-column logical second option.
2. Observe with `--cells` and capture/open a cell screenshot. The first option,
   `Yes, proceed (y)`, is focused; the complete Grid retains the wrapped command.
   Observation must send no input and must not acknowledge render dirty state.
3. Run `bus agent press worker --select 'Yes, proceed (y)' --terminal-id TERMINAL_ID`
   using the observed terminal ID. Require `selected` and no pending selection.
   Wait for the synchronized redraw to finish and observe the ordinary composer.
   The worker becomes idle and history contains `fake reply: fake:dialog-null`.
   Stop the session. The selection receipt alone does not prove this result.
