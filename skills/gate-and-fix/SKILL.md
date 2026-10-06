---
name: gate-and-fix
description: Use on a feature branch that is getting ready to commit, when lint, unit, integration, coverage, and diff gates must pass and failures must be remediated from complete failure evidence (the main agent decides whether to fix them itself or dispatch subagents), committing and pushing round by round until the branch converges.
---

# Gate and Fix

Read [guide.md](guide.md) and [gate-round-format.md](references/gate-round-format.md) completely.

```text
INPUT = [--base <immutable commit>]

repo   = git rev-parse --show-toplevel
branch = git branch --show-current
base   = caller-provided immutable commit, otherwise origin/master

IF branch is empty OR branch IN {main, master}
  STOP "gate-and-fix runs only on a feature branch."
IF git status --porcelain --untracked-files=all is nonempty
  STOP "The worktree is not clean; commit or remove every tracked and untracked change first."
IF git diff --name-only <base>...HEAD is empty
  STOP "There are no committed changes against the base."

branch_slug = branch with every character outside [A-Za-z0-9_-] replaced by `-`
artifact_root = temp/gate-and-fix/<branch_slug>
round = 1
LOOP:
  Run:
    python3 skills/gate-and-fix/scripts/gate_and_fix.py \
      --repo "<repo>" --base "<base>" --round "<round>" \
      --artifact-root "<artifact_root>"

  Capture the runner exit code and its sole stdout line as artifact.
  IF exit code NOT IN {0, 1}
    STOP with stderr; do not read an old artifact.

  Run:
    python3 skills/gate-and-fix/scripts/gate_and_fix.py verify \
      --artifact "<artifact>" --base "<resolved base commit>"
  IF verification fails
    STOP with the artifact; do not infer an outcome from Markdown.

  IF exit code == 0
    Assert verifier output is PASS; BREAK.
  Assert verifier output is FAIL.

  Run:
    python3 skills/gate-and-fix/scripts/gate_and_fix.py list \
      --artifact "<artifact>" --base "<resolved base commit>"
  IF list fails
    STOP with the artifact; do not infer failed gates from Markdown.
  Capture its one-name-per-line stdout as failed gates.

  FOR each listed failed gate and each stream IN {stdout, stderr}
    Run:
      python3 skills/gate-and-fix/scripts/gate_and_fix.py show \
        --artifact "<artifact>" --base "<resolved base commit>" \
        --gate "<gate>" --stream "<stream>"
    Capture its exact stdout as that stream's remediation evidence.

  Group all failed gate evidence by writable owner.

  **How the remediation is orchestrated is main's own decision**: fix sequentially itself,
  dispatch subagents grouped by owner, or mix the two. The basis is how large the failure surface
  is, whether the owners are genuinely disjoint, and how much context main has left: one or two
  small fixes are usually faster to do directly, because dispatching costs a brief and a context
  rebuild; dispatching pays off only when failures span several mutually disjoint owners and every
  group carries substantial work. This skill does not prescribe a shape, nor does it presume that
  parallel is faster.

  Hard constraints when delegating (however many groups): each subagent receives only its own
  group's artifact identity, exact owned paths, and decoded logs; it changes only files inside its
  own allowlist; it performs no Git mutation; it returns its changed files and blockers. Evidence
  with overlapping owners must be in the same group — two agents writing the same file is a STOP,
  not something to coordinate.

  After every remediation has been collected, main inspects the combined delta.

  IF any owner is unsafe/unclear, any group overlaps, a blocker is reported, or no intended
     remediation changed the worktree
    STOP with the artifact and blocker; do not retry an identical tree.

  Invoke `commit-and-push` once. It owns the only Git mutation of this round. Increment round and
  repeat all four mandatory checks plus the diff gate on the clean committed tree. Never run the live
  end-to-end check or only a selected subset in a later round.

RETURN the final PASS artifact, its base/head, and all round commits.
```
