---
name: gate-and-fix
description: Run Bus quality gates through canonical round artifacts, remediate failures from complete evidence, and repeat on a committed tree until the branch converges.
---

# Gate and Fix

Read [guide.md](guide.md) and [gate-round-format.md](references/gate-round-format.md) completely.

```text
INPUT = [--base <immutable commit>]

repo   = git rev-parse --show-toplevel
branch = git branch --show-current
base   = caller-provided immutable commit, otherwise origin/master

IF branch is empty
  STOP "gate-and-fix requires a named branch."
IF git status --porcelain --untracked-files=all is nonempty
  STOP "The worktree must be clean before gate-and-fix starts."
IF git diff --name-only <base>...HEAD is empty
  STOP "There are no committed changes against the base."

branch_slug = branch with characters outside [A-Za-z0-9_-] replaced by -
artifact_root = temp/gate-and-fix/<branch_slug>
round = 1

LOOP:
  Run:
    python3 skills/gate-and-fix/scripts/gate_and_fix.py \
      --repo "<repo>" --base "<base>" --round "<round>" \
      --artifact-root "<artifact_root>"

  Capture the exit code and sole stdout artifact path.
  IF exit code NOT IN {0, 1}
    STOP with stderr; never read an older artifact.

  Run:
    python3 skills/gate-and-fix/scripts/gate_and_fix.py verify \
      --artifact "<artifact>" --base "<resolved base commit>"
  IF verification fails
    STOP with the artifact; do not infer results from Markdown.

  IF runner exit == 0
    Assert verifier output is PASS and BREAK.
  Assert verifier output is FAIL.

  Run:
    python3 skills/gate-and-fix/scripts/gate_and_fix.py list \
      --artifact "<artifact>" --base "<resolved base commit>"
  Capture the exact failed gate names.

  FOR each failed gate and stream in {stdout, stderr}
    Run:
      python3 skills/gate-and-fix/scripts/gate_and_fix.py show \
        --artifact "<artifact>" --base "<resolved base commit>" \
        --gate "<gate>" --stream "<stream>"
    Treat the complete decoded stream as remediation evidence.

  Group failures by writable owner. The main agent may fix a small local group
  directly or delegate genuinely disjoint groups. Every worker receives only
  its artifact identity, decoded logs, and exact owned paths; it performs no Git
  mutation. Overlapping owners are one group, never concurrent writers.

  After remediation, inspect the composed delta. STOP if an owner is unclear,
  groups overlap unsafely, a blocker remains, or the intended repair made no
  tree change. Never retry the same tree.

  Invoke commit-and-push once; it owns the round's sole Git mutation. Increment
  round and rerun the entire applicable gate set on the clean committed tree.

RETURN the final PASS artifact, base/head identities, each round's commits, and
any host/tooling limitation. Never hand-summarize a malformed artifact.
```

The Bus adapter runs `just ci` when both `just` and `cargo-nextest` are
available. Otherwise it expands that recipe into direct Cargo, Python, and Bun
commands, then runs any affected skill-local Python tests and
`git diff --check`. The artifact records the exact path used. It deliberately
does not import Desktop pnpm, coverage, Electron-session, or submodule gates.
