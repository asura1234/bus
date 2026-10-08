# Codex queued question fixtures

Captured from Codex CLI 0.161.0 in a private Bus instance on 2026-10-08,
at 131 columns and 45 rows. The prompt asks only for a native async question;
the workspace and Bus data directory are isolated under /private/tmp.

- collapsed.txt: question and bullet options in the transcript, queued question banner,
  empty main composer; agent dialog returned dialog:null and fingerprint:null.
- expanded.txt: after Shift+Left; numbered choices plus the generated Other row.
- other-selected.txt: after Down twice; Other is focused and accepts text directly.
- other-typed.txt: after typing T; the focused last row now contains T.

Screens retain the real blank lines, wrapping, native key glyphs and working indicator.
