---
name: split-pr
description: Split a multi-purpose Bus branch into single-purpose branches from an explicit base, as parallel PRs, a stacked PR train, or a mix of both, with dependencies proven by git and builds. Restack the branches after a parent changes or lands. Use when asked to split a PR, divide a branch into multiple PRs, stack PRs, or restack a PR train.
---

# Split PR

```text
INPUT $ARGUMENTS = [split] [--base origin/<branch>] [--publish] [--multi-parent wait|merge]
                 | restack [<plan.json>] [--publish]
mode = restack when the first word is `restack`, otherwise split.
--publish: open or update PRs. Without it, branches stay local.

HELPER  H = python3 skills/split-pr/scripts/split_plan.py
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
ERROR if source empty (detached HEAD) or source equals the base branch.
git fetch origin
base_ref = --base or origin/master; ERROR unless it resolves. Never infer it
  from a remote's symbolic HEAD.
base_sha = git merge-base <base_ref> HEAD
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
  Create the part worktree with worktree-new semantics:
    git worktree add -b <branch> <path> <start>
  ERROR if <branch> already exists locally or on origin.
  Reconstruct the part: git cherry-pick <commits>; for hunk-split commits apply
  only this part's hunks and commit them with the original subject. Never drop
  or duplicate a hunk.
  Verify in that worktree:
    git diff --check <start>...HEAD
    cargo check, plus focused tests for the touched modules (unset BUS_* and
    HERDR_* for test runs); for skill or script parts, their python suites.
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

## Publish

```text
Publish in waves. The main agent runs the waves; no script orchestrates them.
WHILE some part is unpublished and not waiting:
  wave = every unpublished part whose parents all have a recorded PR or landed
         (in parallel: all parts in the first wave; in a train: one part per
         wave, bottom first; in a mixed graph: every part whose parents are done)
  Skip parts whose base is a merge (render's Base column) under
  multi_parent == wait: they stay local and verified until it is one branch.
  Before a wave that holds a stacked part, H restack PLAN must report `current`
  (a previous wave's `pr` may have rebased a root); otherwise run Restack mode.
  The parts in a wave are independent PRs: spawn one subagent per part, all in
  the same turn, each in that part's worktree, and wait for all of them.
  Subagents never run `H record` or edit PLAN; each returns its branch tip and
  PR number. After the wave the main agent runs, per part:
    H record PLAN --part <id> --pr <number>   (record moved tips first)
  A failed subagent stops further waves; report its part and blocker.

Subagent task for a root part (no open parent) when base_ref is origin/master:
  invoke `pr` in the part worktree. It rebases onto origin/master and opens the
  Draft PR against master. Return the PR number and whether the rebase moved
  the branch.

Subagent task for a stacked part (one open parent, or a merge base under `merge`),
or a root part when base_ref is not origin/master (`pr` only targets master):
    base_branch = the open parent's branch, <branch>--base under `merge`, or
                  base_ref without `origin/` for a root part
    IF base_branch == <branch>--base:
      ERROR if it exists on origin with a different SHA than recorded onto;
      git push origin <onto>:refs/heads/<branch>--base
    IF git ls-remote --exit-code origin refs/heads/<branch> succeeds:
      ERROR unless that SHA equals the local branch tip (a resumed publish);
    ELSE: git push origin refs/heads/<branch>:refs/heads/<branch>
    Write goal and non-goal files from this part's purpose, then:
      python3 skills/pr/scripts/pr_goal_context.py --branch <branch> \
        --output <ctx> --goal-file <goal> --non-goal-file <non-goals>
    Read skills/pr/references/pr-template.md. Write a Draft body from
    git diff <onto>...<branch> only, with `H stack PLAN --part <id>` output as
    a bullet in 摘要, then:
      python3 skills/pr/scripts/pr_format_check.py --phase draft \
        --template skills/pr/references/pr-template.md --title "<title>" \
        --body-file <body> --goal-context-file <ctx> --locked-goal-file <locked goal>
    existing = gh pr list --head <branch> --base <base_branch> --state open --json number
    IF existing: gh pr edit <number> --title "<title>" --body-file <body>
    ELSE: gh pr create --draft --base <base_branch> --head <branch> --title "<title>" --body-file <body>
    Return the PR number.

After every part is published, set each PR's `- **Stack**:` bullet in 摘要 to
`H stack PLAN --part <id>` (children numbers now exist): read the body with
gh pr view --json body, insert or replace only that line, rerun pr_format_check with the
phase the PR is in, and gh pr edit --body-file.
Run H restack PLAN; when it is not `current`, run Restack mode.
```

## Restack mode

```text
PLAN = argument, else the plan for the current branch's source; ERROR if missing,
  or if no argument was given and its source.branch is not the current branch.
git fetch origin
FOR each part with a PR and landed false:
  IF gh pr view <pr> --json state reports MERGED:
    H record PLAN --part <id> --landed
result = H restack PLAN
IF result.status == current: report and stop.
Show the steps to the user. They rewrite branch history; continue only with
confirmation unless the user asked for the restack explicitly.
FOR step in result.steps (already in dependency order):
  in the part worktree, or a scratch worktree when the part has none, run
  step.commands in order. On conflict: resolve by intent (the part's own
  commits win over nothing; never drop a parent change); if unsure, run
  git rebase --abort and STOP with the conflicting files.
  Verify as in BUILD (diff check, cargo check, focused tests).
  H record PLAN --part <id> [--onto <merge sha> when step.onto_ref is merge(...)]
  IF --publish AND step.push_base present: run step.push_base.
  IF --publish AND step.push present: run step.push.
  IF --publish AND step.retarget present: run step.retarget.
  IF --publish AND the part has no PR yet and is now publishable: publish it
    as in Publish.
Rerun H restack PLAN; it must report `current`.
A part whose parents have all landed is an ordinary branch on the base: when
--publish is given and base_ref is origin/master, invoke `pr` for it to converge
gates and finalize the body; on any other base `pr` would rebase it onto master,
so leave it on its retargeted base.
```

## RETURN

Report:

- shape, the `H render PLAN` table, and the PLAN path;
- per part: branch, base, depends on, source commits or hunks, verification run,
  PR number and URL when published, and whether it waits for parents;
- content left on the source branch and why;
- `H coverage` result from BUILD; it is not rerun after publishing, so a tip
  moved by `pr` (rebase, gate fixes) or by restack is verified by that step's
  own checks, not by coverage;
- every force-push and retarget performed in restack mode.

Keep the source branch until every part is verified and the user explicitly
authorizes cleanup.
