---
name: split-pr
description: Split a multi-purpose repository branch into single-purpose branches from an explicit base, as parallel PRs, a stacked PR train, or a mix of both, with dependencies proven by git and builds. Restack the branches after a parent changes or lands. Use when asked to split a PR, divide a branch into multiple PRs, stack PRs, or restack a PR train.
---
Resolve `SKILL_DIR` to the directory containing this loaded SKILL.md; keep the working directory in the target repository.
# Split PR

```text
INPUT $ARGUMENTS = [split] [--base origin/<branch>] [--publish] [--multi-parent wait|merge]
                 | restack [<plan.json>] [--publish]
mode = restack when the first word is `restack`, otherwise split.
--publish: open or update PRs. Without it, branches stay local.

HELPER  H = python3 "$SKILL_DIR/../split-pr/scripts/split_plan.py"
PLAN    temp/split-pr/<source-branch with / replaced by ->/plan.json

RULES
- Read guide.md before classifying parts or dependencies.
- Read references/split-plan-format.md before writing the plan.
- Edit PLAN by hand only in CLASSIFY and CONFIRM (including a return to CONFIRM
  from BUILD); everywhere else it changes only through `H record`. Never
  hand-edit onto, tip, pr or landed.
- Never push to `upstream`, never push the source or base branch, never
  force-push without `--force-with-lease=<ref>:<expected sha>`.
- Never delete the source branch, a part branch, or a remote branch.
- Preserve unrelated changes: never stash, reset, or switch branches over
  tracked dirty files in the user's worktree. Build parts in separate worktrees.
- Any helper exit code other than 0 stops the run; fix the plan or the branch
  and rerun the helper. Never edit around a failed check.
```

## Split mode

```text
========== PREFLIGHT ==========
repo = git rev-parse --show-toplevel
source = git branch --show-current
ERROR if source empty (detached HEAD).
git fetch origin
IF --base is absent:
  `git remote set-head origin --auto`
  Run repository_context.py from the sibling rebase-origin-main/scripts directory.
  base_ref = context.base with the exact `refs/remotes/` prefix removed.
ELSE: base_ref = caller --base.
Require origin/<branch> per the split-plan format and a resolvable commit; never substitute main/master.
Resolve origin/<branch> as refs/remotes/origin/<branch> for every Git lookup/operation; the plan label stays origin/<branch>.
Run the shared rebase_base.py --base "<full base ref>" --check-branch "<source>" guard; STOP if it fails.
base_sha = git merge-base <full base ref> HEAD
source_sha = git rev-parse HEAD
ERROR if tracked changes are uncommitted: everything to split must be committed.
ERROR if git rev-list --merges <base_sha>..HEAD is nonempty (linearize first).
IF PLAN exists: STOP. Offer restack mode when its source.branch equals source;
  otherwise another branch shares PLAN's slug: report the collision.

========== CLASSIFY ==========
Inspect git log --reverse --stat <base_sha>..HEAD and every commit's diff.
Group commits into parts by purpose (guide.md "Parts"). A commit that serves
two parts is split by hunk; list it in both parts.
For each pair of parts, decide depends_on from evidence (guide.md
"Dependencies are proven, not guessed"). Commits that belong to no part go to
left_on_source with the reason, recorded in PLAN.
Name branches `codex/<topic>` unless the user gave exact names.
multi_parent = --multi-parent or `wait`.
Write PLAN. Run:
  H check PLAN      # schema, coverage of every source commit, cycles
  H probe PLAN      # git replay per part, with and without its parents
WHILE probe reports `missing-dependency`:
  add the dependency named in `overlaps` or regroup; rerun check and probe.
FOR each part with status `build-check-dependency`:
  prove it (guide.md); drop the edge if the part builds and passes alone.
IF a part can only become independent through a redesign:
  STOP and explain; do not invent a split.

========== CONFIRM ==========
Run H render PLAN and show its output verbatim: shape, order, base, depends on,
commits, file count and +/- lines per part, the source total, files per part,
and the mermaid graph. For any part that is still large, offer splitting it
further by sub-purpose (guide.md "Part size"). For a part with two or more parents,
state the policy and its trade-off (guide.md), and offer linearizing when it
fits. Wait for the user's confirmation. Any change: edit PLAN, rerun check,
probe, render, and confirm again.

========== BUILD (dependency order from `H check`) ==========
FOR part in order:
  open = parents not landed
  IF open empty:     start = base_sha
  IF one open:       start = refs/heads/<parent branch>
  IF several open:   in a detached scratch worktree at the first parent:
                       git merge --no-ff -m "chore: integrate <ids> for <id>" <other parent branches>
                     start = that merge commit (keep its SHA as onto)
  ERROR if <branch> already exists locally or on origin.
  git branch --no-track <branch> <start>
  Invoke `worktree-new <branch>`: it checks out the existing branch in a new
  worktree and provisions its target-declared dependencies (plain `git worktree add`
  alone skips that provisioning).
  Reconstruct the part: git cherry-pick <commits>; for hunk-split commits apply
  only this part's hunks and commit them with the original subject. Never drop
  or duplicate a hunk.
  Verify in that worktree:
    git diff --check <start>...HEAD
    target-derived compile checks and focused tests for the touched modules (unset BUS_* and
    HERDR_* for test runs, including retained pane/session compatibility keys);
    for skill or script parts, their python suites.
  IF verification fails because the part needs another part's code:
    the dependency was missed: STOP building, add the edge, back to CONFIRM.
  H record PLAN --part <id> [--onto <merge sha> for several open parents]
Run H coverage PLAN. `fail` means a hunk was dropped, duplicated across parts,
built into the wrong part, or a stray change was added: the union of the part
branches differs from the source tree without the left_on_source commits, the
parts' own diffs (lines and file modes) do not sum to that tree, or a part
without hunk-split commits differs from its commits replayed onto its base. Fix the branch and rerecord.
`review-left-on-source` lists the files the left_on_source commits leave
different from the source. Coverage cannot tell which part a hunk of a
hunk-split commit belongs to (the plan does not record it): for each part with
such a commit, read git diff <onto>...<branch> and confirm every hunk serves
that part's purpose.

IF --publish absent: go to RETURN.
```

## Publish — explicit follow-up pending forge B

Publication is held until the porter adapts this skill to forge B's pr_host interface after it lands.
IF --publish: STOP before any host write; return the verified local split plan and branches, with
"publication pending forge B adapter". Do not use gh directly or implement a second platform adapter.
The follow-up must preserve per-part intent locks, explicit parent/base, Draft state, head read-back,
and dependency-aware publish waves by delegating through forge's documented interfaces.

## Restack mode

```text
PLAN = argument, else the plan for the current branch's source; ERROR if missing,
  or if no argument was given and its source.branch is not the current branch.
git fetch origin
FOR each part in `H check PLAN` order (parents first) with a PR and landed false:
  IF caller-provided exact-head evidence proves the parent landed:
    H record PLAN --part <id> --landed
  ELSE: retain its recorded state. Automatic host refresh is part of the forge B follow-up.
result = H restack PLAN
IF --publish: STOP "publication pending forge B adapter" before any publish/push/retarget step.
IF result.status == current: report and stop.
Show the steps to the user. They rewrite branch history; continue only with
confirmation unless the user asked for the restack explicitly.
FOR step in result.steps (already in dependency order):
  in the part worktree, or one from `worktree-new <branch>` when the part has
  none, run step.commands in order. On conflict: resolve by intent (the part's own
  commits win over nothing; never drop a parent change); if unsure, run
  git rebase --abort and STOP with the conflicting files.
  Verify as in BUILD (diff check and target-derived focused checks).
  H record PLAN --part <id> [--onto <merge sha> when step.onto_ref is merge(...)]
  IF --publish AND step.push_base present: run step.push_base.
  IF --publish AND step.push present: run step.push.
  IF --publish AND step.retarget present: run step.retarget.
  IF --publish AND the part has no PR yet and is now publishable: publish it
    as in Publish.
Rerun H restack PLAN; it must report `current`.
A part whose parents have all landed remains on its recorded integration base. Do not retarget
or rebase it onto a different default branch to accommodate a host tool.
```

## RETURN

Report:

- shape, the `H render PLAN` table, and the PLAN path;
- per part: branch, base, depends on, source commits or hunks, verification run,
  PR number and URL when published, and whether it waits for parents;
- content left on the source branch and why;
- `H coverage` result from BUILD; it is not rerun after publishing, so a tip
  moved by `pr` (rebase) or by restack is verified by that step's
  own checks, not by coverage;
- every force-push and retarget performed in restack mode.

Keep the source branch until every part is verified and the user explicitly
authorizes cleanup.
