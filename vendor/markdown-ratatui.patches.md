# markdown-ratatui local patches

This file tracks intentional local changes applied on top of the vendored
`markdown-ratatui` source. Remove a patch only when the upstream crate contains
an equivalent fix or exposes an option that lets Bus keep the same behavior.

## 0001 record row breaks

status: active

patch: `vendor/patches/markdown-ratatui/0001-record-row-breaks.patch`

upstream: https://github.com/karanabe/mira

upstream pr: none

vendored base: `markdown-ratatui 0.1.0`

local files:

- `vendor/markdown-ratatui/src/layout.rs`
- `vendor/markdown-ratatui/src/lib.rs`

reason: Bus copies selected room history as the rendered text, and must rejoin
the rows the renderer soft-wrapped without inserting spaces or newlines that
were not in the message. The crate kept no record of where it wrapped, so Bus
guessed from row widths and got URLs, long paths, words split mid-token and
explicit prompt newlines wrong. `Layout::row_breaks` now reports, per row,
whether it follows a hard break, a mid-token wrap or a wrap that elided
whitespace, plus the width of its quote/list/code prefix (Claude Code's
`HardBreak` / `Continuation` / `ContinuationElidedSep` model).

The same patch changes where `Builder::text` may wrap. Rows break only between
whitespace-separated tokens (or between wide CJK characters), so a sentence's
final period or a URL fragment never wraps onto a row by itself, and
whitespace that does not fit ends its row instead of starting the next one
with a space.

remove when: upstream `markdown-ratatui` exposes per-row wrap kinds and wraps
at whitespace, or Bus owns its Markdown renderer.

verification:

```sh
python3 -m unittest tools.tests.vendor_markdown_ratatui_test
cargo nextest run history_markdown
```
