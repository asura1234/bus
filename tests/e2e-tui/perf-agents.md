# Sidebar agent counts

## Run contract and setup recipes

- Use a fresh scratch `bus --dev tui` session at 120×40, fake providers only.
  Never use the running Bus, its data directory, `target/debug`, or desktop automation.
- Setup may use the driver's scratch `bus room create` and `bus agent add` commands
  to create rooms and fake Claude workers, with PWD set to the session's scratch
  workspace. Wait until all workers report idle. Read-only scratch state/history may
  corroborate every step.
- Drive navigation, recipient selection, turn submission and Settings through real
  TUI input. Use snapshots and `bus state` for counts; capture and inspect cell
  screenshots for visual assertions. No evidence may be skipped. Always stop the
  scratch session.
- ACTIVE means agents whose shown status is `working`; all other statuses are
  inactive. TOTAL means every agent in that room, including unavailable agents.
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
   to `Agents 0/3`, and both requests have replies in scratch history. Capture an
   inspected screenshot showing the finished count.

**Pass criteria:** ACTIVE counts Working agents only and follows 0 → 1 → 2 → 1 → 0;
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
   records the reply. Capture state and the sidebar.

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
