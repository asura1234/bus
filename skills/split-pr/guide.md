# Split PR judgment guide

## Parts

A part is one independently valuable purpose with its own tests. Group by
purpose, not directory. A purpose that cannot be reviewed without another part's
code is not wrong; it is a dependency, and the plan says so explicitly.

## Part size

Split by purpose first. Size decides whether a purpose is still too big to
review: a smaller review surface exposes more issues, and reviewers (Codex
especially) miss things in large diffs. `H render` reports each part's file
count and +/- lines of its PR diff (its state against its parents' state) so
the user can see that. Numbers marked `~` are rough: the part's ancestry has a
hunk-split commit or does not replay cleanly, so they sum each whole commit.

When a part is large (tens of files or thousands of changed lines), offer a
further split into sub-purposes that are each independently valuable and
tested, usually as a stack: model and contract first, then the consumers, then
cleanup. The user decides; never split a purpose into pieces that cannot be
reviewed or verified alone just to hit a number.

## Dependencies are proven, not guessed

Part X depends on part Y when X modifies code Y introduces, or relies on it
(calls a function, implements a trait, uses a type, extends a test fixture Y
adds). Evidence, strongest first:

1. `probe` replays X with and without its declared ancestors using
   `git merge-tree`. `missing-dependency` means X does not apply even with its
   declared parents: add the parent named in `overlaps`, or regroup.
   `textual-dependency` means X conflicts alone and applies with its parents:
   the dependency is proven.
2. `build-check-dependency` means X applies alone, so the dependency, if real,
   is semantic. Prove it on a scratch branch: X's commits alone on the base
   must fail to build or test (`cargo check`, focused `cargo test`). If X
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

- `wait` (default): publish the part only once its base is a single branch;
  until then it stays a verified local branch. When one parent lands while
  another is still open (a partial fan-in), the open parent predates the
  landed code, so the part's base becomes a merge of `base.ref` and the open
  parents and it keeps waiting. Every published PR shows
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
correct only for parts with no open parent on an `origin/master` base. A stacked
part, or any part of a split from another base, is published by `split-pr`
itself as a Draft PR whose base is its parent branch (or the base branch), with
a body that passes `pr_format_check.py --phase draft`. Once its parents land and
it is restacked onto an `origin/master` base, it is an ordinary branch and `pr`
refreshes its body.

Independent PRs never wait for each other. The publish phase runs in waves of
parts whose parents already have PRs, one subagent per part, so parallel parts
run `pr` concurrently and a train advances one part per wave. The waves are
plain agent instructions, not a script: `split_plan.py` stays a deterministic
helper, and only the main agent writes the plan, after each wave, so
subagents never race on it.

## Restack

After review fixes on a parent, children must be rebased onto the new parent
tip; after a parent squash-merges, children must drop the parent's original
commits with `git rebase --onto <base> <old parent tip>`. The recorded `onto`
makes both mechanical. Restack never chases the base branch for its own sake;
that is `rebase-origin-main` on the root.
