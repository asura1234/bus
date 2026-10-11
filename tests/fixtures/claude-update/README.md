# Claude Code update fixtures

Captured on 2026-10-11 at 120 columns and 40 rows from a copy of the Claude Code
2.1.295 native binary run with a scratch HOME (so its auto-updater wrote only
there), auto-updates enabled and no login. The workspace path is replaced with
/tmp/workspace.

- update-installed-2.1.295.ansi: the raw PTY bytes. The updater installed
  2.1.296 in the background and drew "✔ Update installed · Restart to update"
  in the footer while the prompt stayed live; nothing asked for input.
