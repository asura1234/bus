# Split PR judgment guide

## Parts

A part is one independently valuable purpose with its own tests. Group by
purpose, not directory. A purpose that cannot be reviewed without another part's
code is not wrong; it is a dependency, and the plan says so explicitly.

## Dependencies are proven, not guessed

Part X depends on part Y when X modifies code Y introduces, or relies on it
(calls a function, implements a trait, uses a type, extends a test fixture Y
adds). Evidence, strongest first:

1. Textual: in a scratch worktree at the base, cherry-pick X alone. A conflict
   that disappears once X's declared parents are cherry-picked first proves the
   dependency. A conflict that remains means a parent is missing: `git blame`
   on the conflicting lines at the source tip names the commit, and the plan
   names its part; add that edge, or regroup.
2. Semantic: X applies alone, so any dependency must show in a build. X's
   commits alone on the base must fail `cargo check` or a focused test. If X
   builds and passes alone, drop the dependency: parallel is cheaper to review
   and merge.
3. A part declared independent can still need another semantically. The per
   branch build/test on its own base is the check that catches it.

Prefer fewer edges. Every edge serializes review and merge and adds restack
work. Never add an edge for ordering taste alone.

## Shapes

- Parallel: all parts sit on the base; each PR merges in any order.
- Train: each part sits on the previous part's branch. Review in any order,
  merge strictly bottom up. Never merge a PR into its parent branch; only the
  bottom PR merges, into the base.
- Mixed: a DAG. Parts with one parent stack on it; roots sit on the base.

## A part with two or more parents

GitHub gives a PR exactly one base, so a part that needs both A and B has no
natural base. Both policies build and verify the branch now, on a local merge
of its parents, so the dependency is proven immediately. They differ in
publication:

- `wait` (default): publish the part only once at most one parent is still
  open; until then it stays a verified local branch. Every published PR shows
  exactly its own diff and restacks with a plain `git rebase --onto`. Cost: the
  part's review starts later.
- `merge`: publish now on an integration branch `<branch>--base` that merges the
  open parents, so the diff is clean. Costs: one more pushed branch that every
  parent change must re-merge, and a branch someone could mistakenly merge the
  PR into. Choose it when the human wants the review now.

Linearizing (stacking B on A so D has one parent) is a third option when A and
B are small or naturally ordered; it trades B's independent merge for a plain
train. Offer it in the plan when it fits.

## PR integration

The `pr` skill rebases onto `origin/master` and asserts base `master`, so it is
correct only for parts with no open parent. A stacked part is published by
`split-pr` itself as a Draft PR whose base is its parent branch, with a body
that passes `pr_format_check.py --phase draft`. Once its parents land and it is
restacked onto the base, it is an ordinary branch and `pr` finalizes it.

## Parallel work

Independent parts never wait for each other. Build, publish, and restack in
waves of ready parts, one subagent per part, each in its own worktree; a train
is just a graph whose waves hold one part each. Only the main agent edits
PLAN, after each wave, so subagents never race on it.

## Restack

After review fixes on a parent, children must be rebased onto the new parent
tip; after a parent squash-merges, children must drop the parent's original
commits with `git rebase --onto <base> <old parent tip>`. The `Onto` column in
the plan makes both mechanical. Restack never chases the base branch for its own sake;
that is `rebase-origin-main` on the root.
