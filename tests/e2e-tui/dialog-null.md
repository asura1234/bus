# Soft-wrapped Codex permission dialog

## Run contract

Use a fresh scratch Bus through the dev TUI driver at **80x35**, with all inherited
`BUS_*` and `HERDR_*` variables removed. No live providers or live Bus are allowed.
No SKIP is allowed. The inert `fake:dialog-null` fixture draws a minimized captured
Codex permission screen whose second option soft-wraps before an indented shell
redirection. It never executes the displayed command. Enter closes the fixture.

## Setup

Start the scratch session. Its `status` gives the scratch `work` directory WORK.
CLI setup is allowed:

```text
T bus room create dialog-lab
T bus agent add --room dialog-lab --name worker --provider codex --pwd WORK --consent-hooks
```

Wait for the worker to be idle. Beyond setup, CLI is limited to read-only state,
history, visible agent reads, and the dialog and choose APIs being tested.

### DN-01 — A soft-wrapped permission is readable and answerable

1. Complete setup. Click `room:dialog-lab`, press `ctrl+p`, click `recipient:worker`,
   click `composer`, paste `fake:dialog-null`, and press `enter`, one call per action.
   Wait for the worker's blocked notice. Capture `bus state` and
   `bus agent read worker --source visible`. The visible read contains all three
   permission options, the wrapped second option, and its shell redirection.
   Its viewport columns are below the second option's 122-column width.
2. Run `bus agent dialog worker`. It returns a non-null choice dialog and fingerprint,
   all three options, and option 1 selected. Option 2 retains the complete command
   and `> /tmp/dialog-probe.log 2>&1` continuation. Capture and open a screenshot.
3. Run `bus agent choose worker --option 1 --fingerprint F` using that fingerprint.
   The outcome is `closed`. Wait for the worker to become idle. A fresh dialog read
   returns null; history contains `fake reply: fake:dialog-null`. Stop the session.
