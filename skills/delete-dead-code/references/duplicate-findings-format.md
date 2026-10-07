# Duplicate Findings Format (duplicate track)

The duplicate track produces one artifact per handling component (duplicate groups whose owner sets intersect,
merged into one); the main agent additionally writes `duplicate-reported.md` for DRIFTED / INTENTIONAL groups and
groups whose owners are out of scope. `dead_code_scope.py verify` checks scope by finding-line anchors and checks
each group's self-consistency by group id. The dead-code track's format is in
[dead-code-findings-format.md](./dead-code-findings-format.md).

Artifact path: `temp/delete-dead-code/<branch-slug>/duplicate-<component-slug>.md`; the reported artifact is
`temp/delete-dead-code/<branch-slug>/duplicate-reported.md`.

## Structure

````markdown
# Duplicate Findings

- Component: `<the component's owner set joined with +; the reported artifact writes reported>`
- Track: `duplicate`
- Base: `<immutable base commit SHA; in explicit-directory mode, HEAD at scan time>`
- Outcome: `CLEAN|ACTED|REPORTED`

## Findings

- `<repo-relative path>:<line>` — DUPLICATE[<group id>] — `<symbol>` — <one-sentence description; verdict IDENTICAL|DRIFTED|INTENTIONAL> — CONFIRMED|LIKELY — <equivalence or difference evidence>

## Disposition

- `<repo-relative path>:<line>` — CANONICAL|CONSOLIDATED|KEPT|HANDOFF — <why>

## Verification

- `<command>` — <summary of the real result>
````

A group id is `[A-Za-z0-9_-]+`, reusing the `G1`, `G2`… numbering assigned when candidates were merged in B1; every
member of one group uses the same group id.

## Rules

`verify` enforces these mechanically (`verify_artifact` and `_group_problems`):

- `- Track: \`duplicate\`` is required. Finding kinds may only be `DUPLICATE[<group id>]`; a `DUPLICATE` without a
  group id is rejected. Dispositions may only be `CANONICAL` / `CONSOLIDATED` / `KEPT` / `HANDOFF`; `DELETED` is
  rejected, because standalone dead code belongs to the dead-code track.
- Every group has at least **2** members.
- When a group has any `CONSOLIDATED`, it has **exactly one** `CANONICAL` (the survivor); a `CANONICAL` requires a
  `CONSOLIDATED` (without a consolidation there is no survivor).
- When any member of a group is `LIKELY` (not sure it is equivalent to the other members), the whole group must not
  be `CONSOLIDATED`.
- A `LIKELY` member itself must not be `CONSOLIDATED` either.
- Outcome: `CLEAN` has no finding; `ACTED` has at least one `CONSOLIDATED`; `REPORTED` has findings and no
  `CONSOLIDATED`.
- Findings and dispositions correspond one-to-one by `` `path:line` ``; anchors must be inside this scope, and a
  component artifact (`--unit` given the owner set) must stay inside that component's owner set.

The main agent reviews these in B5 REVIEW (the script cannot judge them):

- `CANONICAL` is the copy production is wired to: confirmed by reading back from production call sites, never by
  exports, docstrings, or what tests point at. When versions coexist, the survivor is the newest version.
- A `CONSOLIDATED` reason states which `CANONICAL` it was repointed to, and the evidence of observable equivalence
  (arguments, defaults, failure contract, boundary clamping, side effects).
- The consolidation adds no forbidden dependency edge and creates no broad shared module.
- A `KEPT` reason must be the concrete difference of a DRIFTED group (a product question) or the rule an INTENTIONAL
  group cites; "compatibility with old peers" is not a rule. Uncertain version/compatibility items use the fixed
  reason "needs developer ruling: version/compat".
- A `HANDOFF` names the out-of-scope owner module and the consolidation to be done.

## Positive example (ACTED)

````markdown
# Duplicate Findings

- Component: `src`
- Track: `duplicate`
- Base: `604d9797f6fab9d48037ca40ce6a3cdbf2d60c2e`
- Outcome: `ACTED`

## Findings

- `src/config/parse.rs:14` — DUPLICATE[G3] — `trim_quotes` — public version; IDENTICAL — CONFIRMED — function bodies are byte-identical
- `src/config/keys.rs:145` — DUPLICATE[G3] — `trim_quotes` — private copy; IDENTICAL — CONFIRMED — function bodies are byte-identical

## Disposition

- `src/config/parse.rs:14` — CANONICAL — most production callers, already `pub(crate)`, `keys.rs` may depend on it
- `src/config/keys.rs:145` — CONSOLIDATED — now uses `parse::trim_quotes`; behavior is byte-for-byte equivalent

## Verification

- `just test-one config::` — passed
````

## Negative examples

- Group `G1` with both members `CONSOLIDATED` and no `CANONICAL`: rejected; nobody can say which one stays.
- A group with one `LIKELY` member consolidated as a whole: rejected.
- A group with a single member: rejected; a duplicate takes at least two copies.
- Marking a consolidated copy `DELETED`: rejected; the duplicate track's action is consolidation, not deletion.
