# Asynchronous acknowledgement form ordering QA

## Run contract and setup recipes

- Use one fresh 120x40 scratch Bus started by `./run tui`; no real providers or live Bus.
- Drive forms through real keyboard input. Use the dev-only `bus ui hold-acks`,
  `bus ui release-acks`, and read-only `bus ui hits`/`bus state` commands for the
  deterministic ordering fixture and corroboration.
- After arming the hold and submitting or requesting navigation, require
  `held_acknowledgements: 1` from `ui hits` before opening the newer form. Do not
  use sleeps or infer a held acknowledgement from elapsed time.
- Capture snapshots and state for every assertion. Any mismatch is a FAIL; this
  focused document has no SKIP case.

### ACK-01 — A held acknowledgement cannot discard newer form input

**Preconditions/setup:** fresh scratch Bus; default `bus` room open; no agents required.

1. Run `bus ui hold-acks`. Press Ctrl+Shift+R, type `held-room`, press Enter, and wait until `ui hits` reports `held_acknowledgements: 1` and state contains `held-room` → the creation succeeded but its `RoomCreated` presentation acknowledgement has not been applied; the original room form is still open and `bus` remains visible.
2. Press Esc, then Ctrl+O, replace the file path with `newer-input.txt` without pressing Enter → the newer Local file path form shows exactly `newer-input.txt`; `held-room` exists in state but `bus` is still the visible room and `held_acknowledgements` remains 1.
3. Run `bus ui release-acks` → the released `RoomCreated` cannot close or edit the newer form: `newer-input.txt` remains exactly visible in the Local file path form, `bus` remains the visible room, and the held count drains to 0.
4. Press Esc. Press Ctrl+Shift+R, type `completed-room`, press Enter → the acknowledgement for this still-current originating form closes it and opens `completed-room`; no acknowledgement remains held.
5. Return to `bus`, run `bus ui hold-acks`, then run dev `bus room focus MASTER` and wait until `ui hits` reports `held_acknowledgements: 1` → focus is queued but not applied and `bus` remains visible. Press Ctrl+O, replace the path with `newer-focus-input.txt` without Enter → that exact newer input is visible and the visible room remains `bus`.
6. Run `bus ui release-acks` → `newer-focus-input.txt` and its form survive and `bus` remains visible. Press Esc, then run dev `bus room focus MASTER` without arming the hold → the positive control opens MASTER with no form and no held acknowledgement.

**Evidence:** `ui hits` held counts, visible form/path text and room headers, scratch state visible room and room IDs, and the trace showing hold before newer input and release afterward.

**Pass criteria:** both stale asynchronous acknowledgements preserve the newer form, fields, and room context; current-form creation and no-form dev focus still navigate normally.
