# Disk space alerts QA

## Run contract and setup

- Use a fresh scratch Bus through `./run tui`, at 160 columns × 50 rows,
  graphics `none`. Never use `--real` or the desktop. Do not use the running
  Bus or `target/debug`.
- Settings changes must use the UI. Read-only `bus settings`, `bus state`
  and `bus history` may corroborate them.
- The driver's dev build uses synthetic free space. The only fixture write
  allowed is a decimal byte count to `disk-free-bytes` in the
  scratch data directory printed by `status`. Do not modify persisted model
  state or manufacture messages. An absent fixture means healthy capacity.
- All expected assertions require captured evidence; a mismatch is FAIL.
  No SKIP allowance. Open each screenshot and judge it.

## Cases

### DISK-01 — Settings steppers and both Bus notices

**Preconditions/setup:** fresh scratch session; default work room `bus`,
no agents or orchestrator. The synthetic free-space fixture is absent.

1. Open Settings → rows `Disk warning (GB free)` and `Disk critical (GB free)` show 20 and 5. Use Down to focus warning and press Right, then Left; focus critical with Down and press Right, then Left → each increases by one and returns to its default. Capture screen text and read-only saved settings.
2. Click warning's ‹ then ›, and critical's › then ‹ → each changes by one and returns to 20/5. Capture a Settings screenshot and saved settings; both rows and their steppers must be readable without overlap.
3. Close Settings. Write `19000000000` (19 GB) to the fixture and wait for the next 60-second sample → the work room displays exactly one `Disk low: 19 GB free. Delete finished worktrees and cargo target dirs before starting new builds.`. Then write `4000000000` (4 GB) and wait for the next sample → exactly one `URGENT: disk almost full (4 GB free). Stop starting builds now and free space immediately: delete merged worktrees and target dirs.` appears. Both are grey `bus` messages addressed to the developer, MASTER has neither, and history corroborates both. Capture and open a screenshot showing both notices, plus sender cells for the grey colour. A fresh session with a direct drop to 4 GB must show only critical and latch both levels; neither level rearms at or below threshold + 1 GB.
