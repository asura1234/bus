# Dialog fingerprint diagnostic captures

Captured on 2026-10-23 in disposable **headless PTYs only**, at 134 columns x
45 rows. No running Bus, live agent pane, or desktop surface was used.

Providers: Codex CLI 0.164.0 and Claude Code 2.1.309 (classic renderer).
Both ran in fresh `/private/tmp/bus-stale-dialog-{codex,claude}` workspaces,
with inherited Bus/Herdr integration variables removed and scratch BUS_DATA_DIR.
No repository files or provider settings were edited by the model.

Codex prompt: ask one `request_user_input_async` question, "Which capture mode
should Bus use?", with Plain screen and ANSI screen, then run `sleep 180`.
Shift+Left opens the native queue. `codex-expanded-1.ansi` shows Working at
1m 03s; `codex-expanded-2.ansi`, eight seconds later, shows Waiting for background
terminal at 1m 11s. `codex-selection-moved.ansi` follows one Down, without Enter.

Claude prompt: ask one AskUserQuestion with the same question and choices, then
wait for the answer. `claude-question-1.ansi` is the open question;
`claude-selection-moved.ansi` follows one Down, without Enter. The two cumulative
raw snapshots taken five seconds apart were byte-identical (7795 bytes each):
Claude emitted no new bytes during that interval. The duplicate second frame
was removed. Only Codex has captured timer/status churn coverage; this recording
does not establish how other Claude renders behave.

The capture helper used `pty.fork` and retained cumulative provider output bytes.
These fixtures export the final observed cells and their ANSI styles through
pyte, with trailing empty cells trimmed. Private device-status queries are
ignored by pyte; the provider receives normal terminal query responses.
Raw output, helper, exporter, provider transcripts and the incident RCA remain
at `/Users/dylanliu/work/bus/temp/rca-stale-dialog/` and are not included here.

The tests distinguish Codex's unrelated status redraw, which preserves the
digest, from cursor movement in both providers, which the existing exact-dialog
contract rejects. They verify hashed-component observation/write diagnostics;
they do not claim that cursor movement caused the historical unexplained
rejection. The hashes omit plaintext but are unsalted; they are not a
confidentiality guarantee.
