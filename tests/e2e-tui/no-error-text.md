# TUI error text regression

## Run contract

Run only on a fresh driver-owned scratch Bus, with fake providers and 180x60 cells.
Use a mode-0700 private driver root and the exact-head copied dev binary. Never use
live Bus data, real provider accounts or global configuration. Record the client
and server run_start build_sha and copied binary hash. No SKIP is allowed.

Fixture operations may inspect scratch state and logs, create rooms/agents, wrap a
scratch fake launcher and temporarily replace the scratch state.json with a directory.
All behavior under test (sending, adding the failing agent, editing and quitting)
is driven through UI keys/clicks. Restore fault fixtures before teardown. Use driver
waits or bounded waits for a named log event; do not retry failed UI steps.

The no-error rule covers Bus-generated diagnostics. Authored messages and the
native provider terminal are their own content; keep the room conversation selected
when judging the Bus chat, composer, sidebar and footer. Scan snapshots plus all
captured Bus output in pty.cast for diagnostic markers. Take a cell screenshot for
each case, open it, and record what is visible. Inspect captured effects for toasts
or terminal notifications. Keep traces, screenshots and logs in the run folder.

## Cases

### ERR-01 — Send to a busy agent

Preconditions/setup: Create a scratch room `busy-errors` and fake Codex worker
`busy-worker` (PWD: the driver's own work folder). Select the room and worker in
the recipient picker. Setup may use the driver bus command; sends use the composer.

1. Type `fake:work 20 busy-first` in the composer and press Enter. Wait until the worker is Working. Capture a full frame and state; no Bus error text is visible.
2. While it is Working, type `busy-followup` and press Enter. Capture full frames and the message status. The followup is queued or joins the running turn; there is no refusal, recovery, toast, composer or footer error text. Wait for the fake replies, capture the final frame, screenshot and effects.

### ERR-02 — Failed launch

Preconditions/setup: Create/select a scratch room `launch-errors`. Wrap its fake
`claude` launcher: preserve ordinary --version/--help probes, but for the actual
launch print `LAUNCH_FAILURE_SENTINEL` and exit 1. Record the wrapper and original
launcher paths; neither may resolve outside this driver's scratch directory.

1. Through the add-agent form, add `failed-worker` with provider Claude and the scratch work folder as PWD. Return to the room view if the native terminal opens. Wait for the agent to become Unavailable. Capture state, the full frame, screenshot and effects. Unavailable is the only failure indication; no startup diagnostic, launch marker, recovery text, chat failure body or footer error is drawn.
2. Inspect the scratch client log and state. The failure cause has agent/room context in the client log; the saved agent remains available for inspection. Restore the launcher, capture another room frame, and prove the launch marker never entered any recorded room frame.

### ERR-03 — Save failure

Preconditions/setup: Use a fresh scratch room `save-errors`. Locate only this
session's state.json from driver status. Move that file to a sibling backup and
create a directory at the same state.json path to force an actual atomic-save
failure. Preserve the original file and record the fault's paths.

1. Type `retained-save-draft` into the composer, then press Ctrl+Q. Wait for `bus.command.failed` or `bus.storage.paused` in the client log and `bus.quit.warning`. Capture a full frame, screenshot and effects. The draft remains; no save error, unsaved warning, force-quit hint, retry hint or storage marker is drawn. The log contains the failed command and cause.
2. Remove only the injected empty state.json directory and restore the saved file. Wait for the force window to expire using the timestamp from `bus.quit.warning`, then press Ctrl+Q to retry saving. Verify successful clean exit and that the saved draft equals `retained-save-draft`; scan the complete recording for error text and retain the logs.
