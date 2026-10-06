# Split plan format

The split plan is the durable artifact of one `split-pr` run. `scripts/split_plan.py`
parses it fail-closed; a later run reads it to restack the branches after a
parent changes or lands.

Path: `temp/split-pr/<source-branch-slug>/plan.json`, where the slug replaces
every `/` in the source branch with `-`. The file is ignored by Git and is
written by the agent once, then updated only through `split_plan.py record`.

## Structure

```json
{
  "schema": "split-pr-plan/1",
  "source": {"branch": "<source branch>", "sha": "<40-hex source tip at plan time>"},
  "base": {"ref": "origin/<branch>", "sha": "<40-hex base commit the split starts from>"},
  "multi_parent": "wait",
  "parts": [
    {
      "id": "<[a-z0-9][a-z0-9-]*>",
      "branch": "<destination branch>",
      "title": "<conventional commit style PR title>",
      "depends_on": ["<part id>"],
      "commits": ["<40-hex source commit>"],
      "onto": null,
      "tip": null,
      "pr": null,
      "landed": false
    }
  ],
  "left_on_source": ["<40-hex source commit>"]
}
```

## Fields

- Top-level keys and part keys are exactly the ones shown; unknown or missing
  keys are errors.
- `base.ref` is an explicit `origin/<branch>` ref. The GitHub base of a root PR
  is `<branch>`.
- `multi_parent` is `wait` or `merge` and applies to every part with two or
  more open parents (see `guide.md`).
- `parts` has at least two entries. `id` and `branch` are unique; `branch` is
  never the source branch or the base branch.
- `depends_on` lists part ids, without duplicates, self-references, or cycles.
  Empty means the part sits on `base.ref`.
- `commits` lists full SHAs from `base.sha..source.sha` in source order. A
  commit listed by several parts is split by hunk and reconstructed by hand;
  `probe` reports such parts as `manual`.
- Every non-merge commit in `base.sha..source.sha` appears in at least one part
  or in `left_on_source`, never both. The source range must not contain merge
  commits.
- `onto` is the commit the part's own commits sit on: the base, the single open
  parent's tip, or the merge commit of the open parents. `tip` is the branch tip
  when last recorded. Both are `null` until `record`; `tip` requires `onto`.
- `pr` is `null` or the PR number. `landed` is `true` only after the PR merged,
  requires `pr`, and requires every parent to be landed.

## Derived values

`split_plan.py` derives and never stores:

- shape: `parallel` (no dependencies), `train` (one root, every part with at
  most one parent and one child), otherwise `mixed`;
- dependency order: Kahn's algorithm, ties in plan order;
- a part's base: `base.ref` when no parent is open, the open parent's branch
  when one is open, otherwise `merge(<parent branches>)`; under `merge` the
  published integration base is `<branch>--base`.

## Malformed examples

- `"base": {"ref": "master", ...}`: base is not an explicit `origin/` ref.
- `"depends_on": ["c"]` on part `a` while `c` depends on `a`: cycle.
- `"landed": true, "pr": null`: landing needs a PR.
- `"commits": ["1a2b3c4"]`: abbreviated SHA.
