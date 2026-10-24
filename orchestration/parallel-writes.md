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
Shared files: <X>: ask <named lock owner> before writing; wait for grant/release
  or <X>: sole owner <worker>; message that owner before editing
PR peers: <PR/task> → owner <name/ID> → verifier <name/ID> → reviewer(s) <name/ID>
Checks / build outputs: <worker>: <commands>; <worktree>/target; <build slot/artifacts>
Build slots: <literal absolute slot-1 path>; <literal absolute slot-2 path>
Integration: <agent>; <merge, stack or rebase order>; conflicts: <agent>
Commit access: <agent or grant order>; stage only owned files
Cleanup: <agent>; <worktrees and target dirs>; <checkpoint>
```

## Shared files and Git operations

Owners whose work touches the same file **message each other before editing**,
including across separate worktrees. Exchange paths, intended edits and heads;
agree who applies the edits and their order. Give the file one owner who applies
the others' proposed edits, or name a lock owner in the map who grants only one
writer access at a time. Ask that owner and wait for an explicit grant; record
the holder in the map. The holder reports its edits and releases the file before
the next grant. A read or request is not a write grant. Shared schemas,
lockfiles and generated files follow the same policy. Ownership-map changes
still need orchestrator approval; routine grants and releases go directly
between the named peers, following the
[peer messaging SOP](orchestrator-guide.md#open-peer-messaging).

Parallel writes do not mean parallel mutations of the same Git index. On a
shared branch, name one agent to commit, or serialize commit access through
the named integration owner. Stage explicit owned files or owned hunks, inspect
the staged diff and commit only that work. Never absorb another worker's changes
into a commit, stash, reset or cleanup.

## When worktrees are needed

Separate worktrees and branches are for parallel authors whose ownership
overlaps so heavily that one branch would need constant hand-offs, or whose
gates/builds conflict through shared outputs, profiles or a required clean tree.
Have the author use `worktree-new`; record each checkout, branch, immutable base
and owner in the map. Authors still obey their ownership maps inside their own
worktrees. Reviews and investigations use the feature author's worktree as
specified below; they do not get another checkout.

Reuse exactly one Cargo target per worktree: export
`CARGO_TARGET_DIR=<worktree>/target` for every Cargo, gate, coverage and E2E run,
and set `CARGO_LLVM_COV_TARGET_DIR` to that same path for coverage. Never make
per-run targets or share a writable target between worktrees. Serialize builds in the author worktree,
including reviewer/investigator probe builds. Each room has two full-build slots:
`/tmp/bus-<room id>-build-slot-1` and `/tmp/bus-<room id>-build-slot-2`.
The orchestrator takes its room id from its prompt, resolves both literal
absolute paths and includes them in each build worker's brief and ownership map.
Workers use the supplied paths; ask the orchestrator if they are missing.
Before a gate, coverage or E2E binary build, atomically acquire one of those
slots with `mkdir`; if both are busy, wait 60 seconds and retry. Record the
owner, expiry and liveness check, and release only your slot with a trap on
success or failure. Follow the
[resource protocol](orchestrator-guide.md#resource-leases-and-build-slots).
Final review, verification and bot review all use the same integrated head
in the feature author's worktree; coordinate access rather than creating another
target or checkout.

## Reviews and investigations

Reviews and investigations are mostly read-only. Their only repository-content
writes are new test files or added cases in existing test files; never edit
production code, configuration, build scripts, docs or plans in that role.
Reports, logs and saved probe patches belong in ignored workflow artifact paths.
An author who needs to investigate before a fix exists first uses `worktree-new`
to create the feature worktree where the fix will be made, then investigates
there. Never build in the main checkout or touch the live Bus or its target.

The orchestrator must not create reviewer/investigator worktrees, diagnostic
clones or scratch repository copies (including `bus-review-*` and `bus-diag-*`).
Reviewers and investigators use the feature author's development worktree and
its single target. Prefer the author's already-built binary for the exact head.
If a probe needs compilation, coordinate with the author and use that target
through the room's build slot; never create a second target. Serialize probe
writes/builds with the owner's gates so a gate sees only its committed head.

Before adding a probe, tell the author its exact test paths and intended cases,
and agree exclusive access to any shared test file. Run the focused probe,
then save a patch containing only your additions, including new untracked test
files, plus the command and red/green output in ignored review artifacts.
Remove only your additions before handing back the worktree, even for green
probes or interrupted reviews; preserve the author's and other reviewers'
changes. Do not stage, commit, stash or reset the shared tree to do this.
Report patch paths and prove the affected test paths have returned to their
pre-probe state. The author independently verifies the evidence and chooses
whether to adopt/rewrite the probe or leave the saved patch unadopted. A red
probe must neither dirty the owner's gate nor enter its commit accidentally.

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
- Message affected shared-file owners before editing. Send proposed edits to
  the sole owner, or ask the named lock owner and wait for the grant. Report
  completion directly to those peers and release the lock.
- On a conflict or unexpected overlapping edit, stop writing the affected file,
  preserve your work and report the paths, branch/head and conflicting changes
  directly to the named integration/file owner and affected peers. Escalate
  ownership changes or unresolved conflicts to the orchestrator; do not resolve
  it by discarding, resetting or overwriting someone else's work.
- Commit only your own files or owned hunks, with the granted commit access.
  Inspect the staged diff; leave other workers' changes untouched. Report your
  changed paths, commit/head, checks and any outstanding locks or blockers.
- Use the PR peers named in your brief: send each pushed head to the verifier
  and reviewers; verifiers send `VERIFIED` or defects with `file:line` directly
  to the owner. Owners use `auto-merge-pr`'s owner-run path: at most two fix
  rounds, no out-of-goal repairs, no unresolved FLAG/developer decision, and
  re-measure against its 3000-line split threshold before merge. Send follow-ups,
  flags and exhausted-budget blockers to the orchestrator in finals.
- Report a missing or replaced peer to the orchestrator, which owns room notes,
  the roster and verifier allocation. Workers do not edit the notes or roster.
  Finals still go to the orchestrator. Send only the message body; Bus adds
  the header, so do not sign messages.

## Clean up after integration

Record a cleanup checkpoint and agent in the workflow. After the required
integration/merge checkpoint, an agent **outside the target worktree** runs
`worktree-close`. First prove its changes are committed, pushed and integrated
by content, and archive required evidence outside it. Preserve dirty or unpushed
work and worktrees retained for later steps; report them instead of forcing removal.
Keep remote branches unless the developer asked to delete them.

Delete inactive per-run targets you created now and after each gate/E2E; retain
only the worktree's single `target` until that worktree closes. Archive required
evidence first and prove no build/check uses the paths. Never remove the live
Bus target or another worker's output. Log deleted absolute paths and measured
sizes, release the build slot on success or failure, and record retained paths
and worktree/branch cleanup in the workflow.

## Repository status reports

Repository monitoring and Git/host operations belong to the worker agent and
its harness. For assigned work, report a PR/merge request becoming ready for
review, completed CI, a new conflict, its merge, and any upstream merge that
materially affects the assignment. Include the repository, PR number, exact
head/status and next action. Send one update per meaningful transition to the
room's bound orchestrator; if none exists, use `bus send --room ROOM --as WORKER
--to human --text "STATUS"` to post in your own room for You. The installed
`how-to-bus-cli.md` documents this explicit recipient. Do not monitor or broadcast
unrelated repository activity, and do not expect Bus to interpret repository
workflow state for you.
