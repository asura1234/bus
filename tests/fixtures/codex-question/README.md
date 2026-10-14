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

Captured from Codex CLI 0.162.0 on 2026-10-14 in a 134x45 PTY (rendered with pyte, trailing
spaces trimmed), while the turn kept working after `request_user_input_async`:

- collapsed-warning.txt: queued question banner while Working, with a background terminal in the
  working line and a startup warning right-aligned on the `? for shortcuts` footer row. Live room
  agents showed the same footer; Bus missed the question there.
- expanded-working.txt: the same question after Shift+Left, still Working above the queue.
- queued-message-hides-question.txt: a pending question plus one steered and one Tab-queued
  message. Codex hides the question while messages are queued, and Shift+Left edits the last
  queued message instead of opening the question.
