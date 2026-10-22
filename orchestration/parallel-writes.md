# Parallel writes

This guide is for orchestrators and workers changing several places at once.
The [orchestrator rules](orchestrator-rules.md) bind the orchestrator; every
parallel worker receives this guide and its ownership map in its brief.

Bus installs this file as `<BUS_DATA_DIR>/docs/parallel-writes.md`, beside
`orchestrator-rules.md` and `orchestrator-guide.md`. At the default data root,
that is `~/.local/share/bus/sessions/<id>/docs/parallel-writes.md`. The
orchestrator resolves the **absolute session path** from its prompt's References
and puts that actual path in every parallel brief. Workers read the installed
file directly; they do not need the orchestrator's prompt or a source checkout.
The path remains the same for that session across worker hand-offs and resumes.

## Before parallel work starts

Split a change that spans several places across agents running in parallel.
Use the acceptance checklist to assign an item or group to each worker. Strongly
encourage parallel subagents for investigation and fixes; the parent worker
subdivides its own write area and remains responsible for it. Order dependent
steps explicitly.

Default to one shared branch. Before dispatch, write an ownership map by
directory or file in the workflow's Coordination section and copy the relevant
map into **each brief**, along with the instruction to read this installed doc
at its resolved absolute path. Name the repository, branch, owned paths,
shared-file policy, check/build output directories and integration owner.
Only an orchestrator-approved update to the map changes write ownership; tell
every affected worker before it takes effect. A worker's subagents cannot
expand that worker's area.

Use this short template; replace every placeholder before sending:

```text
Read first: <absolute session docs path>/parallel-writes.md
Repository / branch: <absolute checkout path> / <branch>
Acceptance items: <checklist item IDs and checks>
Ownership:
  <worker A>: <directory/** or exact files>
  <worker B>: <directory/** or exact files>
Shared files: <X>: ask <orchestrator> before writing; wait for grant/release
  or <X>: sole owner <worker>; send proposed edits to that owner
Checks / build outputs: <worker>: <commands and exclusive target/artifact dirs>
Integration: <agent>; <merge, stack or rebase order>; conflicts: <agent>
Commit access: <agent or grant order>; stage only owned files
Cleanup: <agent>; <worktrees and target dirs>; <checkpoint>
```

## Shared files and Git operations

Give a shared file one owner who applies the others' proposed edits, or tell
each writer: **"Ask me before writing file X and wait."** The orchestrator
grants only one writer access at a time and records the holder in the map.
The holder reports its edits and releases the file before the next grant. A
read or a request is not a write grant. Shared schemas, lockfiles, generated
files and ownership-map edits follow the same policy.

Parallel writes do not mean parallel mutations of the same Git index. On a
shared branch, name one agent to commit, or serialize commit access through
the orchestrator. Stage explicit owned files or owned hunks, inspect the staged
diff and commit only that work. Never absorb another worker's changes into a
commit, stash, reset or cleanup.

## When worktrees are needed

Switch to separate worktrees and branches when ownership overlaps so heavily
that one branch would need constant hand-offs, or when gates/builds conflict
through shared outputs, profiles or a required clean tree. Have an agent use
`worktree-new`; record each checkout, branch, immutable base and owner in the
map. Workers still obey their ownership maps inside their own worktrees.

Assign each build/check an exclusive target and artifact directory. Set
`CARGO_TARGET_DIR` away from the live Bus repository's target directory; separate
worktrees must not point it at the same writable outputs. Serialize checks
that require the same resources. Final review, verification and bot review
all use the same integrated head, with isolated outputs where needed.

## Integrate deliberately

Before workers start, name an integration agent and record the dependency
order and strategy in the workflow:

- **Merge:** independent branches share an immutable base; integrate them into
  the feature branch in the recorded order.
- **Stack:** dependent branches build on their predecessor; integrate parent
  first, then restack children when the parent changes or lands.
- **Rebase:** record the exact target and branch order; update dependent
  branches after their parent, using the repository's rebase skill.

The orchestrator coordinates; the integration agent performs Git operations and
owns conflict resolution. It consults the affected file owners before resolving
behavioral conflicts and investigates technical uncertainty. Goal changes and
product decisions without a basis for a recommendation go to the orchestrator's
developer decision boundary. Workers report conflicts rather than independently
rewriting other branches. Preserve each worker's changes, then run required
gates and concurrent review/verification/bot review on the integrated head.
Any further integration or fix changes the head and needs fresh evidence.
Use `auto-merge-pr` with its recorded workflow authorization and explicit holds;
do not add a new per-PR merge OK.

## For workers

- Read the installed doc at the absolute path in your brief and your ownership
  map before editing. If either is missing, ask the orchestrator.
- Write only inside your assigned area, including when using subagents. Request
  an ownership change before touching another area.
- Ask the orchestrator before writing a shared file and wait for the grant, or
  send proposed edits to its sole owner. Report completion and release the lock.
- On a conflict or unexpected overlapping edit, stop writing the affected file,
  preserve your work and report the paths, branch/head and conflicting changes
  to the orchestrator. Wait for the named integration/file owner; do not resolve
  it by discarding, resetting or overwriting someone else's work.
- Commit only your own files or owned hunks, with the granted commit access.
  Inspect the staged diff; leave other workers' changes untouched. Report your
  changed paths, commit/head, checks and any outstanding locks or blockers.

## Clean up after integration

Record a cleanup checkpoint and agent in the workflow. After the required
integration/merge checkpoint, an agent **outside the target worktree** runs
`worktree-close`. First prove its changes are committed, pushed and integrated
by content, and archive required evidence outside it. Preserve dirty or unpushed
work and worktrees retained for later steps; report them instead of forcing removal.
Keep remote branches unless the developer asked to delete them.

After no build/check process uses them and evidence is archived, remove only
the regenerable target/artifact directories assigned to this effort, including
any external `CARGO_TARGET_DIR`. Name their absolute paths before cleanup;
never remove the live Bus target directory or another worker's output. Log the
worktree/branch cleanup and any retained paths in the workflow.
