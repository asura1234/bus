# TUI driver dialog suite

`suite.json` presents 15 sequential prompts through the scriptable fake provider.
Send `RUN_DIALOG_DRIVER_SUITE` once. It ends with
`TUI_DRIVER_DIALOGS_CLEARED:15` only after every prescribed answer is accepted.
Each completed prompt appends `{id, answer, passed}` to the fake provider's
private callback directory as `fake-dialog-results.jsonl`. A wrong answer records
`passed:false` and stops the fake provider; no choice executes an actual command.

Tell the real driver agent plainly: **We are testing the TUI driver. Every dialog
is from the fake harness in a private scratch Bus and runs nothing real. Clear
each with the tool using the answers below, and report any dialog the tool could
not read or answer.** Give it the fake pane identity and this answer table.

| Prompt ID | Prescribed answer | Input shape |
| --- | --- | --- |
| claude-update | Continue with current version | Arrow menu; starts on Install |
| codex-update | Update now (runs `npm install -g @openai/codex`) | Arrow menu |
| codex-command | Yes, proceed (y) | Arrow menu |
| codex-permission | 1 | Immediate number key |
| kimi-compaction | Continue as-is | Unnumbered arrow menu; starts on Compact |
| kimi-folder-trust | Trust and continue | Unnumbered arrow menu |
| codex-folder-trust | Yes, trust this directory | Arrow menu |
| codex-hook-review | Trust all and continue | Arrow menu; starts on Review |
| numbered-tool-approval | 2 | Immediate number key |
| typed-project-name | tui-driver-demo, then Enter | Free text |
| typed-codex-answer | ANSI screen, then Enter | Free text |
| oh-my-zsh-update | n, without Enter | Immediate character key |
| shell-yes-no | yes, then Enter | Shell line input |
| pager | q | Immediate character key |
| keyboard-shortcut | Ctrl+L | Arbitrary chord |

The Codex update wording and options derive from the captured native
[`chooser-0.162.txt`](../codex-update/chooser-0.162.txt); the surrounding README
records its capture provenance. The Kimi folder-trust screen derives from a real
Kimi 2.4.1 startup captured at Bus head `50e53ea1` on 2026-11-01; only the path and
option rendering are normalized. The remaining screens are **modeled fixtures** using
ordinary documented prompt shapes and existing Bus dialog fixtures/parsers. They
are not claimed to be fresh native provider captures. In particular, the existing
Claude `update-installed-2.1.295.ansi` capture is a passive footer, so the
interactive Claude update screen here is explicitly modeled.

`oh-my-zsh.zsh` reproduces the ordinary oh-my-zsh update question using actual zsh
`read -k 1`. Run `/bin/zsh -f <absolute path>` in a real scratch Bus shell pane to
exercise a prompt outside an agent; expected terminal output is
`OH_MY_ZSH_ANSWER:n`. This fixture performs no update. The fake-provider copy alone
does not prove the real-shell acceptance requirement.

Run the opt-in real-agent acceptance from the feature checkout with a separately
copied, exact-head binary built with `dev-tools`:

```sh
BUS_DIALOG_DRIVER_BINARY=/absolute/path/to/copied/bus \
BUS_DIALOG_DRIVER_SHA=$(git rev-parse HEAD) \
python3 tests/dialog_driver_acceptance_test.py -v
```

The runner uses the approved real Kimi installation at `~/.kimi-code/bin/kimi`
and real HOME, with a fresh private `BUS_DATA_DIR` (0700). It does not copy or edit
the credential store; Bus's provider adapter retains its normal shared refresh
locks. It starts the scratch Bus with `--dev --yolo` so the real driver's own tool
approval does not add another prompt to this driver test. It checks the copied
binary's hash, the current source HEAD, and both client/server full build SHAs
before adding agents or sending input. The same isolated `sbus` wrapper controls
the private server throughout.

The runner handles only the real driver's initial native folder-trust setup.
The real Kimi agent answers all 15 fake prompts and the actual zsh pane using
`agent observe` / `agent press`. A pass requires all 15 unique audit entries to
be true, the fake completion marker, `OH_MY_ZSH_ANSWER:n` from the shell pane, and
the driver's explicit correlated completion reply. Set
`BUS_DIALOG_DRIVER_TIMEOUT` to change the default 600-second task deadline.

Selected evidence is saved under `temp/tui-driver/acceptance/<full-head>/` before
the private server is stopped and its scratch directory removed. An unconfirmed
stop retains the scratch directory and records its path. Native trust setup may
leave the ordinary throwaway project trust entry through Kimi itself; this runner
does not edit global provider configuration to remove it.

The added `screen` action uses `id`, `screen` and `input`. Input mode `menu`
specifies `labels`, zero-based `answer` and `selected`, optional `marker` (default
`›`) and `numbered` (default true). The screen places rows with `{{options}}`.
Modes `text`, `number` and `key` specify an exact `answer`; text renders the draft
at `{{answer}}` and completes on Enter. Number and key complete immediately.
