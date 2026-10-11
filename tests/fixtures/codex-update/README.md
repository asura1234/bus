# Codex startup update fixtures

Captured from Codex CLI 0.162.0 (npm install) on 2026-10-10 at 120 columns and
40 rows, with an isolated CODEX_HOME whose version.json named 0.162.1 as the
latest release. The workspace path is replaced with /tmp/workspace.

- chooser-0.162.ansi: the raw PTY bytes from Codex's last full clear onward. Codex
  first draws its main composer, then redraws the screen as the update chooser.
- chooser-0.162.txt: the same screen as plain text.

Older Codex releases drew "Update available!" with a "Press enter to continue"
footer; this release draws "Update available ·" with an "enter continue · esc
skip" footer and three options.
