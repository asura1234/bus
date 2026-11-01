# Sitrep format

## Computed state

`marker.py read --repo REPO` writes one JSON object with exactly `default_ref`,
`head`, `finished`, and `open_prs`. `default_ref` is `origin/` plus the default
branch, and `head` is its current 40-hex commit. Save this object unchanged for
the renderer.

`finished` is the ordered result of `git log SAVED..DEFAULT_REF`. Each entry
has exactly `name` (the commit subject, including any `(#N)` in a squash merge)
and `pr` (`landed` plus the commit's 40-hex SHA). The agent cannot add, drop,
reorder, or rename these entries.

`open_prs` is the mechanically listed sequence of open PR links. Each string is
`[#N title](https://example.com/pr/N)`. Work without a PR is not in this array.

Without a saved commit, `marker.py read` fails unless `--start` supplies an
explicit 40-hex starting commit. There is no implicit 24-hour or other guessed
starting point.

## Report input

The input to `render.py --state STATE.json` is one JSON object with exactly
`blockers`, `ongoing`, `finished`, `deferred`. Each is an array; empty arrays
render as `None`. Every string is nonempty, one line, and contains no control
characters. `blockers` holds hard blockers and bottlenecks slowing work as
plain strings; an empty list is the only way to render `None`. Each entry must
include evidence. A blocker states what is stopping work or waiting on the
developer; a bottleneck states what is slowing work, such as a build-slot queue,
review round, or disk pressure. Never put caveats or unrelated commentary in
that section.

The other arrays contain objects with exactly these keys:

- `ongoing`: `name`, `tldr`, `context`, `status`, `pr`.
- `finished`: `name`, `tldr`, `context`, `pr`.
- `deferred`: `name`, `tldr`, `context`.

The report's Finished entries must equal the computed state's entries in the
same order and with the same `name` and `pr`. The agent supplies only their
TLDR and context. The sequence of PR links in Ongoing must equal the state's
`open_prs`; work without a PR uses `not opened yet (branch: BRANCH)` and may be
interleaved with those entries. Supply the evidenced branch, or `unknown` when
it is unavailable. Any computed-state mismatch, extra item, omitted item, or
reordering is rejected.

`status` is exactly `planning`, `in-development`, `test`, `review`, or
`blocked`. In a blocked task, context names the dependency or developer action.
Escape literal backslashes and unmatched brackets in PR titles for Markdown.

The renderer outputs these headings in this order. Task fields are nested
bullets; TLDR is one line and Context gives the full known context. Notes on
missing evidence belong in a relevant task's Context, not the Blockers and bottlenecks section
or an extra section.

```text
## Blockers and bottlenecks
None

## Ongoing
- Task name
  - TLDR: One-line summary.
  - Context: Full evidenced context.
  - Status: in-development
  - PR/MR: [#12 Title](https://example.com/pr/12)

## Finished (since the last sitrep)
- Completed task (#11)
  - TLDR: One-line summary.
  - Context: Full evidenced context.
  - PR/MR: landed 7cd03ec7a69d4881223643e68064f69edc385369

## Deferred
- Deferred task
  - TLDR: One-line summary.
  - Context: Full evidenced context.
```

Malformed examples include extra commentary in Blockers and bottlenecks, a Finished item that
does not match the computed Git log, an omitted or reordered open PR, multiline
fields, omitted sections, unknown status values, and bare PR URLs.

## Repository marker

The marker is `<main-worktree>/temp/sitrep/last-sitrep-commit`. The main
worktree is resolved from `git rev-parse --git-common-dir`, including when the
command starts in a linked worktree. The file contains exactly one 40-hex
commit and a trailing newline. `marker.py write --repo REPO --state STATE.json`
saves exactly the computed state's `head` without refetching, only after that
state was rendered. A commit landing between read and write therefore remains
in the next sitrep's Git log. A missing or malformed marker is an error, not an
implicit first run.
