# Split plan format

The split plan is the durable artifact of one `split-pr` run. `scripts/split_plan.py`
parses it fail-closed; a later run reads it to restack the branches after a
parent changes or lands.

Path: `temp/split-pr/<source-branch-slug>/plan.json`, where the slug replaces
every `/` in the source branch with `-`. Distinct branches can share a slug
(`feat/a-b`, `feat/a/b`), so `source.branch` is the plan's identity: a plan
whose `source.branch` is not the branch being split is a collision, never a
plan to reuse. The file is ignored by Git and is
edited by hand only while classifying and confirming the split; after that it
changes only through `split_plan.py record`, which alone writes `onto`, `tip`,
`pr` and `landed`.

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
  "left_on_source": [{"sha": "<40-hex source commit>", "reason": "<why it stays on the source>"}]
}
```

## Fields

- Top-level keys and part keys are exactly the ones shown; unknown or missing
  keys are errors.
- `base.ref` is an explicit `origin/<branch>` ref. The GitHub base of a root PR
  is `<branch>`.
- `multi_parent` is `wait` or `merge` and applies to every part whose base is
  a merge: two or more open parents, or a partial fan-in (see `guide.md`).
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
- `left_on_source` entries have exactly `sha` (full SHA, no duplicates) and a
  non-empty `reason`, so a later restack can still report why the commit stayed.
- `onto` is the commit the part's own commits sit on: the base, the single open
  parent's tip, or the merge commit of its base refs (the open parents, plus
  `base.ref` in a partial fan-in). `tip` is the branch tip
  when last recorded. Both are `null` until `record`; `tip` requires `onto`.
- `pr` is `null` or the PR number; non-null numbers are unique across parts. `landed` is `true` only after the PR merged,
  requires `pr`, and requires every parent to be landed.

## Derived values

`split_plan.py` derives and never stores:

- shape: `parallel` (no dependencies), `train` (one root, every part with at
  most one parent and one child), otherwise `mixed`;
- dependency order: Kahn's algorithm, ties in plan order;
- a part's base: `base.ref` when no parent is open, the open parent's branch
  when it is the only parent, otherwise `merge(<refs>)` of the open parent
  branches, led by `base.ref` when some parent already landed (a partial
  fan-in: the open parent predates the landed code); under `merge` the
  published integration base is `<branch>--base`.

## Malformed examples

- `"base": {"ref": "master", ...}`: base is not an explicit `origin/` ref.
- `"depends_on": ["c"]` on part `a` while `c` depends on `a`: cycle.
- `"landed": true, "pr": null`: landing needs a PR.
- `"commits": ["1a2b3c4"]`: abbreviated SHA.
- `"left_on_source": ["<sha>"]`: a left commit without its reason.
