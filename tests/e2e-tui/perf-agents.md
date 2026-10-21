# Dev performance overlay agent counts

## Run contract and setup recipes

- Use a fresh scratch `bus --dev tui` session at 120×40, fake providers only.
  Never use the running Bus, its data directory, `target/debug`, or desktop automation.
- Setup may use the driver's scratch `bus room create` and `bus agent add` commands
  to create rooms `perf-a`, `perf-b` and fake Claude workers `alpha`, `beta`, one
  per room, with PWD set to the session's scratch workspace. Wait until both
  workers report idle. Read-only scratch state/history may corroborate every step.
- Drive Settings, navigation and turn submission through real TUI input. Use
  snapshots and `bus state` for counts; capture and inspect cell screenshots for
  visual assertions. No evidence may be skipped. Always stop the scratch session.
- Running means the same shown status `working` that `bus state` reports. Live
  means any shown status except `unavailable`, including launching and blocked.

## Cases

### PERF-01 — Counts follow turns across rooms

**Preconditions/setup:** the two idle workers described above; default room `bus`
open; overlay initially off. Record the scratch settings file before toggling.

1. Open Settings and click Performance overlay. Close Settings. Within 2 seconds,
   row 0 shows `AGENTS 0 running / 2 live` beside the existing CPU, MEM and FPS
   metrics, in the same black-on-yellow strip. Capture and inspect a screenshot.
2. Open perf-a, select only alpha in the recipient picker (Ctrl+P), close it,
   type `fake:work 60 perf-alpha` and press Enter. Wait for alpha to report
   working and row 0 to show `AGENTS 1 running / 2 live`. Capture scratch state
   and a screenshot, inspecting the count and the unchanged metric styling.
3. Switch to perf-b while alpha is still working. Row 0 still shows
   `AGENTS 1 running / 2 live`, proving the count includes the other room.
   Select only beta, submit `fake:work 15 perf-beta`, and wait for both agents
   to report working and `AGENTS 2 running / 2 live`. Capture state and an
   inspected screenshot showing the concurrent count.
4. Wait for beta's turn to finish while alpha is still working. Scratch state
   reports beta idle and alpha working, and row 0 shows
   `AGENTS 1 running / 2 live`. Capture state and row 0.
5. Wait for alpha's turn to finish. Both agents report idle and row 0 returns to
   `AGENTS 0 running / 2 live`; both requests have replies in scratch history.
   Capture state and an inspected screenshot showing the finished count.
6. Open Settings and turn Performance overlay off, then close Settings. Within
   2 seconds the overlay is absent; the top row draws normally. Capture and
   inspect a screenshot. The persisted settings file equals its pre-toggle
   contents: the overlay remains client-local.

**Pass criteria:** counts agree with scratch `bus state`, rise from 0 to 1 to 2
across rooms, return through 1 to 0 as turns finish, and disappear when disabled.
CPU, memory and FPS retain their existing presentation.
