# Dead Code Findings Format (dead-code track)

Each module agent of the dead-code track produces one findings artifact for its module. `dead_code_scope.py verify`
checks scope by the finding-line anchors defined here: **only finding lines take part in the check**; paths mentioned
in prose are not findings. The duplicate track's artifact format is in
[duplicate-findings-format.md](./duplicate-findings-format.md).

Artifact path: `temp/delete-dead-code/<branch-slug>/dead-<module-slug>.md`, where `<module-slug>` is the module name
with `/` replaced by `-`; `<repository-root>` is written as `repository-root`.

## Structure

````markdown
# Dead Code Findings

- Module: `<module name, verbatim from the scope output>`
- Track: `dead`
- Base: `<immutable base commit SHA; in explicit-directory mode, HEAD at scan time>`
- Outcome: `CLEAN|ACTED|REPORTED`

## Findings

- `<repo-relative path>:<line>` — DEAD-BRANCH|DEAD-CODE — `<symbol>` — <one-sentence description> — CONFIRMED|LIKELY — <grep used to confirm>

## Disposition

- `<repo-relative path>:<line>` — DELETED|KEPT|HANDOFF — <why; KEPT names the keeping mechanism, HANDOFF must name the blocking module and what must change there>

## Verification

- `<command>` — <summary of the real result>
````

## Rules

- `- Track: \`dead\`` is required. This track only deletes: finding kinds may only be `DEAD-CODE` / `DEAD-BRANCH`,
  and dispositions only `DELETED` / `KEPT` / `HANDOFF`. `verify` rejects `DUPLICATE`, `CONSOLIDATED`, or `CANONICAL`:
  a consolidation mixed into the deletion track gets waved through as a deletion during review. Duplicates where both
  copies are alive are not recorded here; the duplicate track discovers them.
- The three Outcomes are mutually exclusive and judged mechanically by `verify`, not self-reported:
  - `CLEAN` — no finding at all. `## Findings` and `## Disposition` must both be `- None`.
  - `ACTED` — at least one `DELETED`.
  - `REPORTED` — there are findings, but none was deleted (cross-module, LIKELY, needs a developer ruling).
  **Why `REPORTED` exists**: without it, "found ten real dead-code items but could act on none" could only be
  recorded as `CLEAN`, and the summary would report a problem module as clean. This really happened in the first
  dogfood round.
- `HANDOFF` means the deletion crosses out of this module: the symbol is mine, but its only caller, test, the baseline
  pinning it, or the producer still emitting the old shape is somewhere else. Name the target module; the main agent
  pairs these after the fan-out. **Never write a HANDOFF as KEPT to make the Outcome look better.**
- A finding line must start with `` `path:line` `` (leading whitespace and a `-`/`*` list marker are allowed). This is
  verify's only anchor; any other shape means the finding escapes the scope check, which is a format error.
- Paths are always repository-relative with POSIX separators, never absolute.
- `CONFIRMED` means a whole-repository grep confirmed no references (dead branch: confirmed no producer), and the
  evidence lists the greps actually run; `LIKELY` means no references were found but the code may be reached through
  strings, dynamic dispatch, another language, or a platform condition.
- **Only `CONFIRMED` may be `DELETED`. `LIKELY` is always `KEPT`**, and the main agent summarizes and reports it.
- A `KEPT` reason must name a concrete mechanism (a real injection seam / platform gating / intentional duplication
  pinned by a boundary test / a generated contract / optionality required by the current contract); "to be safe" is
  not accepted. For uncertain version/compatibility items the reason is fixed as
  "needs developer ruling: version/compat", and the main agent builds the developer-ruling list from it.
- The same `path:line` appears exactly once in `## Findings` and once in `## Disposition`, one-to-one; `verify`
  rejects an anchor that appears on only one side.
- A qualifier may follow the confidence field, but `CONFIRMED` / `LIKELY` itself must come right after a `— `.
- `## Verification` lists **every file this agent changed** and the commands actually run. In a shared worktree the
  whole-tree diff already contains other units' changes, so out-of-bounds writes can only be judged from this
  self-report; an unreported file will not be staged at landing either.
- `verify` uses `--unit` to restrict anchors to the artifact's own unit: sibling units of the same invocation are out
  of scope too.

## Positive example (CLEAN)

````markdown
# Dead Code Findings

- Module: `src`
- Track: `dead`
- Base: `844205cb5df4303378daafa917c74f1770749df4`
- Outcome: `CLEAN`

## Findings

- None

## Disposition

- None

## Verification

- No file changed; no verification run
````

## Negative examples

- No `- Track:` line, or `duplicate` with dead-code findings: `verify` rejects it.
- `Outcome: CLEAN` with findings listed: state and content contradict each other; `verify` rejects it and suggests
  `REPORTED`.
- `Outcome: ACTED` without a single `DELETED`, or `REPORTED` that deleted something: rejected as well.
- A finding anchor without a Disposition (or the reverse): rejected; a missing disposition means the finding has no
  conclusion.
- A finding line written as `- src/client/render.rs:12 — DEAD-CODE …` (no backticks): verify's anchor does not match
  it, so the finding effectively escapes the scope check.
- `LIKELY` with `DELETED`: an unconfirmed deletion, rejected.
- Recording two live duplicates as `CONSOLIDATED` in the dead track: rejected; it belongs to the duplicate track.
