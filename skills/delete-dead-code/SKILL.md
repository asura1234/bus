---
name: delete-dead-code
description: Find and remove dead first-party Bus code, zombie tests, or duplicate implementations within a mechanically derived PR scope or explicit directories, with canonical artifacts and boundary verification.
---

# Delete Dead Code

Read [guide.md](guide.md) and [Dead Code Findings Format](references/dead-code-findings-format.md) completely before deleting anything.

```text
INPUT = [--base <immutable commit>] [--directories <dir>...] [--max-parallel <N, default 5>]

repo   = git rev-parse --show-toplevel
branch = git branch --show-current
base   = caller-provided immutable commit, otherwise origin/master

PR-diff scope and explicit-directory scope are mutually exclusive. Explicit
directories replace scope discovery; they do not relax boundary verification.

IF branch is empty
  STOP "delete-dead-code requires a named branch."
IF branch is master AND the developer did not explicitly authorize protected-
branch changes
  STOP and request a feature branch or explicit master authorization.
IF git status --porcelain --untracked-files=all is nonempty
  STOP "Start from a clean tree so this workflow's deletions are attributable."

artifact_root = temp/delete-dead-code/<sanitized-branch>

========== 1. SCOPE ==========

PR scope:
  python3 skills/delete-dead-code/scripts/dead_code_scope.py scope \
    --repo "<repo>" --base "<base>"
Explicit scope:
  python3 skills/delete-dead-code/scripts/dead_code_scope.py scope \
    --repo "<repo>" --directories <dir> <dir> ...

Exit 2 means the scope cannot be derived; STOP rather than guessing. The output
is the complete modifiable unit list and each unit's tracked directories and
exclusions. A finding outside this scope is reported for separate work, never
deleted now.

========== 2. PLAN BATCHES ==========

Run the matching mode:
  python3 skills/delete-dead-code/scripts/dead_code_scope.py batches \
    --repo "<repo>" <scope args> --max-parallel "<N>"

Use the returned rounds exactly. Rounds are serial; units within one round may
be scanned concurrently only when their owners are disjoint.

========== 3. SCAN AND ACT ==========

For every unit, the worker receives exactly:
  - its one-unit allowlist and the artifact path;
  - the script-provided directories and excludes verbatim;
  - this guide and the artifact format;
  - the immutable base;
  - no-Git, no-dev-session, and no-shared-config constraints.

It inventories definitions and consumers in both directions. Trace dead
definitions from symbol to consumer, and dead value branches from producers to
the branch. Explicitly inspect Rust module exports, traits, macros, cfg gates,
serialization strings, include_* assets, command/protocol names, plugin
discovery, tests, and build manifests. Production wiring determines the
canonical implementation; exports, docs, and tests do not override it.

Delete or consolidate only CONFIRMED findings. Keep reachable, compatibility,
dynamic, platform, and intentionally redundant code. Record incomplete cases
as LIKELY/KEPT or HANDOFF. A behavior-changing consolidation is a product
change, not cleanup.

When a unit changed, run its narrow Rust/Python/Bun tests and scoped rustfmt for
changed Rust files. Do not run full coverage, E2E, or whole-tree convergence in
the worker.

After each artifact:
  python3 skills/delete-dead-code/scripts/dead_code_scope.py verify \
    --repo "<repo>" <scope args> --artifact "<artifact>"

STOP on malformed artifacts, out-of-scope edits, blockers, or shared-config
needs. Never let a later round conceal an earlier violation.

========== 3b. HANDOFF ==========

Group HANDOFF findings by the exact pair of involved in-scope units. One worker
may receive the union of that pair solely for the listed symbols and must run
both units' tests. Findings whose blocking owner is outside scope remain
follow-up candidates; do not expand scope or relabel them KEEP.

Verify every handoff artifact with the same command.

========== 4. MAIN REVIEW ==========

The main agent performs the non-delegable consolidation-direction review:
  python3 skills/delete-dead-code/scripts/dead_code_scope.py review \
    --repo "<repo>" <scope args> --artifact <every artifact>...

For every CONSOLIDATED item, trace backward from production call sites and
prove the surviving implementation preserves the previously wired observable
behavior, including arguments, defaults, failure contracts, and bounds. For
every rewritten assertion, require an artifact rationale tied to production
facts. STOP and undo/rework a reversed consolidation; a green test suite cannot
prove the chosen direction was correct.

========== 5. CONSOLIDATE AND VERIFY ==========

Inspect the combined status and prove all edits remain inside scope. If every
unit is CLEAN and the tree is unchanged, report that no confirmed dead code was
found and return.

Run scoped rustfmt, the affected narrow tests, then `just lint`. Use `just ci`
when preparing a PR or crossing integration boundaries. Coverage and live
terminal acceptance remain later workflow gates unless directly required.

========== 6. LAND ==========

Invoke commit-and-push once; it owns the workflow's sole Git mutation.

Return unit/round counts, each artifact and outcome, deletions and
consolidations, every LIKELY/KEPT item, out-of-scope follow-up candidates,
unrun platform/live checks, and landed commit identities.
```
