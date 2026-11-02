# Reordered failed Kimi turns

Minimized from the read-only 2026-11-03 RCA in
`bus-fix-kimi-working/temp/kimi-working/prior-request-active-rca.md`.
The physical event order, native IDs and timestamps are retained. Private
prompt bodies are replaced with `Bus task` / `Background notification`, tool
traffic and earlier guidance are omitted, and the structured error message is reduced to its quota
classification. No credentials or OAuth data are included.

The source report and sanitized event extracts are archived outside the
source worktree at `.git/bus/kimi-failed-turn/prior-request-active-capture`
in the common Git directory, with a SHA256 manifest.

- rr-owner: Bus message 100169 / request 100170; outer failure and owned
  prompt completion precede the delayed agent-end record in the file.
- cc-owner: Bus message 100171 / request 100172; the next autonomous turn
  starts before the previous owned outer failure/completion and delayed end.

Agent iteration IDs and outer turn IDs are independent domains. The matching
numbers in these captures do not establish a general ID equality rule.
