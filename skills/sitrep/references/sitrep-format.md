# Sitrep format

The input to `render.py` is one JSON object with exactly these keys: `blockers`,
`ongoing`, `finished`, `deferred`. Each is an array; empty arrays render as
`None`. Every string is nonempty, one line, and contains no control characters.
`blockers` holds only actual blockers as plain strings; an empty list is the only
way to render `None`. Each blocker must state what is stopping work or what is
waiting on the developer. Never put caveats or non-blockers in that section.

The other arrays contain objects with exactly these keys:

- `ongoing`: `name`, `tldr`, `context`, `status`, `pr`.
- `finished`: `name`, `tldr`, `context`, `pr`.
- `deferred`: `name`, `tldr`, `context`.

`status` is exactly `planning`, `in-development`, `test`, `review`, or
`blocked`. In a blocked task, context names the dependency or developer action.
`pr` is a Markdown link `[#N title](url)` for an opened PR/MR. Escape literal
backslashes and unmatched brackets in titles for Markdown. If none exists,
use `not opened yet (branch: BRANCH)` and supply the evidenced branch; if the
branch is unknown, use `not opened yet (branch: unknown)`.

The renderer outputs these headings in this order. Task fields are nested
bullets; TLDR is one line and Context gives the full known context. Notes on
missing evidence and the initial 24-hour window belong in a relevant task's
Context, not the Blockers section or an extra section.

```text
## Blockers
None

## Ongoing
- Task name
  - TLDR: One-line summary.
  - Context: Full evidenced context.
  - Status: in-development
  - PR/MR: [#12 Title](https://example.com/pr/12)

## Finished (since the last sitrep)
- Completed task
  - TLDR: One-line summary.
  - Context: Full evidenced context.
  - PR/MR: [#11 Title](https://example.com/pr/11)

## Deferred
- Deferred task
  - TLDR: One-line summary.
  - Context: Full evidenced context.
```

Malformed examples include `None for you right now, but ...` in Blockers,
multiline fields, omitted sections, unknown status values, and bare PR URLs.
