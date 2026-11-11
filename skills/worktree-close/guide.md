# Worktree close principles

A provider session depends on its original launch directory for its entire life.
Shell `cd`, a tool's working-directory override, or running Git from main does not
move that dependency. Live cwd handles are operational dependencies, including
those held by Idle reviewers. They are not inert cleanup leftovers.

The launch-directory guard belongs to `scripts/worktree_remove.py`. It reads the
Bus CLI on PATH with `bus state`; the verified schema exposes saved launch paths
as `result.agents[].cwd`. Every agent still in that roster holds its directory,
regardless of activity or pending deletion. The helper compares resolved path
ancestry with filesystem identity, so symlinks, case aliases and nested launch
directories receive the same protection without conflating distinct directories
on case-sensitive volumes.
The helper also checks the caller's supplied original launch directory.

Do not bypass a refusal with raw Git, `rm`, force, a shell cwd change, or an
exception for an Idle agent. Finish authorized non-destructive cleanup and report
the final result while the session can still deliver it. Ask the orchestrator to
collect the results, delete every named holder, and then arrange removal from an
unaffected session. Scheduling deletion or merely ending a turn does not retire
an agent; its roster entry must be gone before removal.

When Bus is absent or unreachable, the helper warns and preserves the existing
closeout checks, including the caller's own launch-directory protection. A
reachable response with malformed roster data or another command error blocks
removal. A clean roster is a current snapshot, not a reservation: coordinate
closeout so nobody launches a new agent in the target while it is being removed.

`--check` performs a non-destructive preflight. Without `--check`, the helper
refreshes the roster and invokes Git only after the guards pass. `--force` only
forwards the previously authorized Git flag; it never overrides the guard. Exit
zero means the requested check or removal succeeded; nonzero means stop. Keep
warnings and refusals visible in the closeout report.
