# Sidebar agent counts

## Run contract and setup recipes

- Use a fresh scratch `bus --dev tui` session at 120×40 unless a case specifies
  another size, fake providers only.
  Never use the running Bus, its data directory, `target/debug`, or desktop automation.
- Setup may use the driver's scratch `bus room create` and `bus agent add` commands
  to create rooms and fake Claude workers, with PWD set to the session's scratch
  workspace. Wait until all workers report idle. Read-only scratch state/history may
  corroborate every step.
- Drive navigation, recipient selection, turn submission and Settings through real
  TUI input. Use snapshots and `bus state` for counts; capture and inspect cell
  screenshots for visual assertions. No evidence may be skipped. Always stop the
  scratch session.
- ACTIVE means agents whose displayed status is `Working` or `Blocked`; `Idle`
  and `Unavailable` are inactive. TOTAL means every agent in that room, including
  unavailable agents.
  Every room has an `Agents` row beneath its name with `ACTIVE/TOTAL` right-aligned
  to the sidebar.

## Cases

### SIDEBAR-01 — Counts follow turns in a several-agent room

**Preconditions/setup:** create `count-a` with idle fake Claude workers `alpha`,
`beta` and `gamma`, and `count-b` with idle worker `delta`; default room `bus` open.

1. Inspect the sidebar → MASTER shows `Agents 0/0`, `bus` shows `Agents 0/0`,
   `count-a` shows `Agents 0/3`, and `count-b` shows `Agents 0/1`. Each count is
   right-aligned on the row directly beneath its room. Capture scratch state and
   an inspected screenshot.
2. Open `count-a`, select only alpha, type `fake:work 60 sidebar-alpha` and press
   Enter. Wait for alpha to report working → `count-a` changes to `Agents 1/3`
   while the other room counts stay unchanged. Capture state and an inspected
   screenshot.
3. Select only beta, submit `fake:work 15 sidebar-beta`, and wait for alpha and
   beta to report working → `count-a` shows `Agents 2/3`; gamma remains idle and
   included in TOTAL. Capture state and the sidebar.
4. Wait for beta's turn to finish while alpha is still working → scratch state
   reports beta idle and alpha working, and `count-a` returns to `Agents 1/3`.
5. Wait for alpha's turn to finish → all three workers are idle, `count-a` returns
   to `Agents 0/3`, and both requests are complete with `stage:ended`, `reply:null` and no failure reason in scratch history. Capture an
   inspected screenshot showing the finished count.

**Pass criteria:** ACTIVE counts Working and Blocked agents and follows 0 → 1 → 2 → 1 → 0;
TOTAL remains three throughout, including idle agents, and other rooms are unchanged.

### SIDEBAR-02 — MASTER has its own live count

**Preconditions/setup:** fresh scratch session; use scratch CLI setup to add fake
Claude orchestrator `master-alpha` in MASTER with `--orchestrates bus`; wait for idle.

1. Inspect the sidebar → the row directly beneath `# MASTER` reads `Agents 0/1`,
   right-aligned, while `bus` still reads `Agents 0/0`. Capture state and an
   inspected screenshot.
2. Open MASTER, select only master-alpha, submit `fake:work 30 master-count`, and
   wait for working → MASTER changes to `Agents 1/1` and `bus` remains `0/0`.
3. Wait for the turn to finish → MASTER returns to `Agents 0/1`; scratch history
   records the completed `ended` request with `reply:null` and no failure reason. Capture state and the sidebar.

**Pass criteria:** MASTER renders the same count row as work rooms and its ACTIVE
value follows its own orchestrator without changing another room.

### SIDEBAR-03 — Counts stay aligned at narrow sidebar widths

**Preconditions/setup:** fresh scratch session; create `narrow-count` with three
idle fake Claude workers; open that room.

1. At the default 120×40 size and sidebar width 28, inspect `narrow-count` → its
   second row contains `Agents` at the left and `0/3` flush right, with blank space
   between them. Capture and inspect a screenshot.
2. Open Settings, set `Sidebar width (20–60)` to 20, close Settings, and resize to
   60×40 → the count row still contains separate `Agents` and `0/3` text within the
   sidebar, right-aligned without overlap or clipping. Capture geometry and an
   inspected screenshot.
3. Reopen Settings, set the sidebar width to 60, resize to 180×40, and close
   Settings → `Agents 0/3` remains on one row beneath the room with the count flush
   to the wider sidebar's right edge. Capture and inspect a screenshot.

**Pass criteria:** the second row remains readable and right-aligned at widths 20,
28 and 60, including a narrow terminal, without overlapping room or agent content.

### SIDEBAR-04 — Blocked agents stay active

**Preconditions/setup:** fresh scratch session; create `blocked-count` with one
idle fake Claude worker `waiting`; open that room.

1. Select only waiting, submit `fake:dialog 15 blocked-count`, and wait until the
   worker is Blocked → the room status is Blocked and its count reads `Agents 1/1`.
   Capture scratch state and an inspected screenshot.
2. Wait for the dialog turn to finish → waiting becomes Idle, the room status is
   Idle, and its count returns to `Agents 0/1`. Capture state, ended/null request history and an
   inspected screenshot.

**Pass criteria:** a Blocked worker is ACTIVE until it returns to Idle.

### SIDEBAR-05 — Performance overlay contains only performance metrics

**Preconditions/setup:** fresh scratch session; create `overlay-count` with one
idle fake Claude worker; default room `bus` open.

1. Open Settings and enable `Performance overlay (CPU, memory, FPS)`, then close
   Settings → the yellow overlay at the top right shows CPU, MEM and FPS on one
   row, without an agents row; the sidebar still shows `Agents 0/1` beneath
   overlay-count. Capture the top two rows and an inspected screenshot.
2. Open Settings, disable the performance overlay, then close Settings → the
   overlay is gone and the sidebar's `Agents 0/1` row remains. Capture the top two
   rows and an inspected screenshot.

**Pass criteria:** agent counts remain in the sidebar independently of the overlay.


### SIDEBAR-06 — Opening an offscreen room reveals its count

**Preconditions/setup:** fresh scratch session at 100×12; use the scratch CLI to
create `extra-1` through `extra-20`; default room `bus` open, no workers needed.

1. Press Ctrl+Shift+R, wait for the `Room name` form, type `final-room` and
   press Enter → the new room is created and
   its main pane title reads `# final-room`. Capture state and an inspected
   screenshot.
2. Press F6 to open the current room → both its selected sidebar name/status and
   the `Agents 0/0` row fit within sidebar body rows 0–8. Named hits contain four
   `room:final-room` targets: name, status, Agents and 0/0. Capture the hits and an
   inspected screenshot.

**Pass criteria:** opening the offscreen room makes its complete two-row entry
visible without additional manual scrolling.

### SIDEBAR-07 — A sidebar that fits exactly does not scroll

**Preconditions/setup:** fresh scratch session at 120×20; create work room
`fit-extra` and fake Claude orchestrator `fit-alpha` in MASTER with
`--orchestrates bus`; wait for idle and open MASTER. Its 17-row sidebar body fits
MASTER, two work rooms, their counts and the orchestrator card exactly.

1. Inspect MASTER → MASTER shows `Agents 0/1`, bus and fit-extra show `Agents 0/0`,
   and the full orchestrator card fits above the footer. Capture named hit
   positions and an inspected screenshot.
2. Scroll down three notches over the sidebar → room and agent hit positions do
   not move; the name/count rows and full orchestrator card remain visible.
   Capture the hits and an inspected screenshot.

**Pass criteria:** a wheel event cannot move an already fully visible sidebar by
an extra blank row.
