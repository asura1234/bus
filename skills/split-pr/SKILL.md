---
name: split-pr
description: Split a multi-purpose Bus branch into single-purpose branches from an explicit base, as parallel PRs, a stacked PR train, or a mix of both, with dependencies proven by git and builds. Restack the branches after a parent changes or lands. Use when asked to split a PR, divide a branch into multiple PRs, stack PRs, or restack a PR train.
---

# Split PR

```text
INPUT $ARGUMENTS = [split] [--base origin/<branch>] [--publish] [--multi-parent wait|merge]
                 | restack [--publish]
mode = restack when the first word is `restack`, otherwise split.
--publish: open or update PRs. Without it, branches stay local.

PLAN = temp/split-pr/<source branch with / replaced by ->/plan.md (ignored by Git)

RULES
- Read guide.md before classifying parts or dependencies.
- Everything is plain git, cargo, gh, and the `pr` skill. No helper script.
- Never push to `upstream`, never push the source or base branch, never
  force-push without `--force-with-lease=<ref>:<expected sha>`.
- Never delete the source branch, a part branch, or a remote branch.
- Preserve unrelated changes: never stash, reset, or switch branches over
  tracked dirty files in the user's worktree. Build parts in separate worktrees.
- Every SHA written to PLAN is a full 40-character SHA read from git.
```

## Plan

PLAN holds the source branch and SHA, `base_ref` and `base_sha`, the
multi-parent policy, a list of commits left on the source with reasons, and one
table row per part, in dependency order:

| Part | Branch | Depends on | Commits | Onto | PR | Landed |
|---|---|---|---|---|---|---|

`Onto` is the commit the part's own commits sit on: `base_sha`, its single
open parent's tip, or the merge commit of its open parents. It is empty until
the branch is built. A later run restacks from it, so update `Onto` and `PR`
every time a branch is built, rebased, or published.

A part is **ready** when every part in its `Depends on` has a built branch (for
building) or an open PR (for publishing). Shape: no dependencies is
**parallel**; one chain is a **train**; anything else is **mixed**.

## Split mode

```text
========== PREFLIGHT ==========
source = git branch --show-current
ERROR if source empty (detached HEAD) or source equals the base branch.
git fetch origin
base_ref = --base or origin/master; ERROR unless it resolves. Never infer it
  from a remote's symbolic HEAD.
base_sha = git merge-base <base_ref> HEAD;  source_sha = git rev-parse HEAD
ERROR if tracked changes are uncommitted: everything to split must be committed.
ERROR if git rev-list --merges <base_sha>..HEAD is nonempty (linearize first).
ERROR if PLAN exists: offer restack mode instead.

========== CLASSIFY ==========
Inspect git log --reverse --stat <base_sha>..HEAD and every commit's diff.
Group commits into parts by purpose (guide.md "Parts"). A commit that serves
two parts is split by hunk; list it in both parts.
Every commit in <base_sha>..HEAD belongs to a part or to the left-on-source
list, never both.
Decide each part's dependencies with git and build evidence (guide.md
"Dependencies are proven"): cherry-pick the part alone onto base_sha in a
detached scratch worktree, then with its parents; build where it applies.
Remove every scratch worktree afterwards with git worktree remove.
Name branches `codex/<topic>` unless the user gave exact names.
multi_parent = --multi-parent or `wait`.
Write PLAN.
IF a part can only become independent through a redesign:
  STOP and explain; do not invent a split.

========== CONFIRM ==========
Show the shape, the PLAN table with each part's base (base_ref, parent branch,
or merge of parents), the files per part, and a mermaid graph:
  flowchart BT; each part points to its parents, roots point to base_ref.
For a part with two or more parents, state the policy and its trade-off
(guide.md) and offer linearizing when it fits.
Wait for the user's confirmation. Any change: update PLAN and confirm again.

========== BUILD ==========
Build in waves. Each wave is every ready, unbuilt part; parts in one wave are
independent, so build them with parallel subagents, one per part, each in its
own worktree. A child starts only after its parents' branches exist.
Per part:
  open = parents not landed
  IF open empty:   start = base_sha
  IF one open:     start = the parent branch tip
  IF several:      in a detached scratch worktree at the first parent:
                     git merge --no-ff -m "chore: integrate <ids> for <id>" <other parents>
                   start = that merge commit
  ERROR if the branch exists locally or on origin.
  git worktree add -b <branch> <path> <start>     (worktree-new semantics)
  git cherry-pick <commits>; for hunk-split commits apply only this part's
  hunks and commit them with the original subject. Never drop or duplicate a hunk.
  Verify in that worktree: git diff --check <start>...HEAD, cargo check, and
  focused tests for the touched modules (unset BUS_* and HERDR_* for tests);
  python suites for skill or script parts.
  IF it fails because it needs another part's code: the dependency was missed;
    STOP the waves, add the edge, back to CONFIRM.
  Report <start SHA> and the branch tip; after the wave the main agent
  records Onto in PLAN (subagents never edit PLAN).

========== COVERAGE ==========
In a detached scratch worktree at base_sha, git merge --no-edit every part
branch in dependency order, then git diff --stat <source_sha>.
The diff must be empty, or touch only files of left-on-source commits.
Otherwise a hunk was dropped or duplicated: fix that branch and rerun.
Remove the scratch worktree.

IF --publish absent: go to RETURN.
```

## Publish

```text
Publish in waves, like BUILD. Each wave is every unpublished part whose
parents all have PRs (or landed). Parts in one wave run as parallel subagents,
one per PR; a train therefore publishes one part per wave, in order.

Root part (no open parent):
  the subagent invokes `pr` in the part's worktree. `pr` rebases onto
  origin/master and opens the Draft PR against master.
  IF the rebase moved the branch: update Onto; its children are now stale, so
  run Restack mode for them before their wave.
Stacked part (one open parent, or several under `merge`):
  base_branch = the parent branch, or <branch>--base under `merge`
  `pr` cannot be used: it rebases onto origin/master and requires base master.
  IF base_branch is <branch>--base: push the Onto merge commit to it,
    refusing if it exists on origin at another SHA.
  ERROR if git ls-remote --exit-code origin refs/heads/<branch> succeeds.
  git push origin refs/heads/<branch>:refs/heads/<branch>
  Write goal and non-goal files for this part, then run
    python3 skills/pr/scripts/pr_goal_context.py --branch <branch> \
      --output <ctx> --goal-file <goal> --non-goal-file <non-goals>
  Read skills/pr/references/pr-template.md. Write a Draft body from
  git diff <onto>...<branch> only, then run
    python3 skills/pr/scripts/pr_format_check.py --phase draft \
      --template skills/pr/references/pr-template.md --title "<title>" \
      --body-file <body> --goal-context-file <ctx> --locked-goal-file <locked goal>
  gh pr create --draft --base <base_branch> --head <branch> --title "<title>" --body-file <body>
Several open parents under `wait`: skip; the branch stays local and verified.
After each wave the main agent records every new PR number in PLAN.

After the last wave, add a stack bullet to 摘要 in every PR (gh pr view --json
body, insert or replace the line, rerun pr_format_check with the PR's phase,
gh pr edit --body-file):
  parallel: - **Stack**: independent part <i>/<n> of the `<source>` split; base `master`; merges in any order.
  stacked:  - **Stack**: part <i>/<n> of the `<source>` split (<shape>); base `<parent branch>`;
            depends on #<parent PR>; merge after it. Stacked on this: #<child PRs>.
  <i> is the part's row in PLAN (dependency order).
```

## Restack mode

```text
Read PLAN for the current source; ERROR if missing.
git fetch origin
FOR each part with a PR and not landed:
  IF gh pr view <pr> --json state reports MERGED: mark Landed in PLAN.
FOR each unlanded part, in dependency order:
  open = parents not landed
  new base = base_ref (open empty), the parent branch (one open), or a new
    merge of the open parent branches (several)
  stale when: a parent was restacked in this run; or one open parent whose tip
    is not Onto; or several open parents that differ from Onto's merge parents;
    or no open parent and Onto is not an ancestor of base_ref (a parent was
    squash-merged).
Show the stale parts and their new bases; they rewrite history, so continue
only with confirmation unless the user asked for the restack explicitly.
Restack in waves: a stale part goes once its parents are restacked; parts in
one wave run as parallel subagents. Per part, in its worktree:
  git rebase --onto <new base> <Onto> <branch>
  (several parents: build the merge commit first, as in BUILD, and under
   `merge` move <branch>--base to it)
  On conflict: resolve by intent, never dropping a parent change; if unsure,
  git rebase --abort and STOP with the conflicting files.
  Verify as in BUILD, then update Onto.
  IF --publish and the part has a PR:
    git push --force-with-lease=refs/heads/<branch>:<old tip> origin refs/heads/<branch>:refs/heads/<branch>
    (under `merge` also push <branch>--base with a lease on the old Onto)
    IF a parent landed: gh pr edit <pr> --base <new base branch>
  IF --publish and the part just became publishable: publish it as above.
A part whose parents all landed is an ordinary branch on master: with
--publish, invoke `pr` for it to converge gates and finalize the body.
```

## RETURN

Report:

- shape, the PLAN table and graph, and the PLAN path;
- per part: branch, base, depends on, source commits or hunks, verification
  run, PR number and URL when published, and whether it waits for parents;
- content left on the source branch and why, and the coverage result;
- every force-push and retarget performed in restack mode.

Keep the source branch until every part is verified and the user explicitly
authorizes cleanup.
