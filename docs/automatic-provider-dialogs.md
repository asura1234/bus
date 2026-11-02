# Answering blocked provider screens

Bus reports a steady blocked screen once per episode. The room's orchestrator
reads it with `bus agent observe AGENT` and decides which visible control to use.
Shell prompts and pagers use the same driver with `--pane`.

Use `bus agent press AGENT --select 'complete visible label'` for a focused list,
`--key KEY` for an explicit control, and `--text 'answer' --submit` for a typed
answer. Bind the press to the observed terminal with `--terminal-id ID` when
needed. Read the returned screen and verify the program's resulting state.

The driver sends one list-navigation key per observed focus change. A pending or
uncertain result preserves the input already queued; inspect it before continuing.
Observation never opens a hidden question panel or answers a prompt. The former
provider-parser hash allowlist is retired; these screens follow the same
orchestrator decision path as any other blocker.
