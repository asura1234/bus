---
name: gate-and-fix
description: Use on a feature branch that is getting ready to commit, when the repository gate command and diff check must pass and failures must be remediated from complete failure evidence (the main agent decides whether to fix them itself or dispatch subagents), committing and pushing round by round until the branch converges.
---
Resolve `SKILL_DIR` to the directory containing this loaded SKILL.md; keep the working directory in the target repository.
# Gate and Fix

Read [guide.md](guide.md) and [gate-round-format.md](references/gate-round-format.md) completely.

```text
INPUT = [--base <immutable commit>] [--command "<repo gate command>"]

repo   = git rev-parse --show-toplevel
branch = git branch --show-current
IF caller base is absent: run `git fetch origin` before refreshing its default.
base   = caller-provided immutable commit, otherwise resolve origin's default branch with:
  python3 "<SKILL_DIR>/../rebase-origin-main/scripts/rebase_base.py" --repo "<repo>" --refresh-default
STOP if base resolution fails; never assume master/main.
command = caller-provided --command, otherwise policy.gate_command from the fresh repo scan.
The runner owns scanning; missing configuration is STOP "no gate configured", never PASS.

Run the branch guard (it uses the full origin symbolic ref, never --short):
  python3 "<SKILL_DIR>/../rebase-origin-main/scripts/rebase_base.py" --repo "<repo>" --base HEAD --check-branch "<branch>"
IF it fails: STOP with stderr.
IF git status --porcelain --untracked-files=all is nonempty
  STOP "The worktree is not clean; commit or remove every tracked and untracked change first."
IF git diff --name-only <base>...HEAD is empty
  STOP "There are no committed changes against the base."

branch_slug = branch with every character outside [A-Za-z0-9_-] replaced by `-`
artifact_root = <absolute Git common directory>/bus/gate-and-fix/<branch_slug>
round = 1
LOOP:
  Run:
    python3 "$SKILL_DIR/../gate-and-fix/scripts/gate_and_fix.py" \
      --repo "<repo>" --base "<base>" --round "<round>" \
      --artifact-root "<artifact_root>" [--command "<caller command>"]

  Capture the runner exit code and its sole stdout line as artifact.
  IF exit code NOT IN {0, 1}
    STOP with stderr; do not read an old artifact.

  Run:
    python3 "$SKILL_DIR/../gate-and-fix/scripts/gate_and_fix.py" verify \
      --artifact "<artifact>" --base "<resolved base commit>"
  IF verification fails
    STOP with the artifact; do not infer an outcome from Markdown.

  IF exit code == 0
    Assert verifier output is PASS; BREAK.
  Assert verifier output is FAIL.

  Run:
    python3 "$SKILL_DIR/../gate-and-fix/scripts/gate_and_fix.py" list \
      --artifact "<artifact>" --base "<resolved base commit>"
  IF list fails
    STOP with the artifact; do not infer failed gates from Markdown.
  Capture its one-name-per-line stdout as failed gates.

  FOR each listed failed gate and each stream IN {stdout, stderr}
    Run:
      python3 "$SKILL_DIR/../gate-and-fix/scripts/gate_and_fix.py" show \
        --artifact "<artifact>" --base "<resolved base commit>" \
        --gate "<gate>" --stream "<stream>"
    Capture its exact stdout as that stream's remediation evidence.

  Group all failed gate evidence by writable owner.

  For a coverage failure, read and apply code-review-guide "Never pad coverage with a tautology"
  through guide.md "Coverage-failure remediation" before choosing a repair.
  IF the coverage gap is reported rather than repaired
    STOP with the FAIL artifact and the reason; reporting does not waive the gate.

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
  repeat the complete profile-selected gate set plus the diff gate on the clean committed tree. For the Bus adapter, never run the live
  end-to-end check here. Never run only a selected subset in a later round.

RETURN the final PASS artifact, its base/head, selected gate command, and all round commits.
A missing gate is a reported configuration blocker, never successful verification.
```
