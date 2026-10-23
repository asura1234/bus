# Diagnose rejected dialog fingerprints

`stale_or_changed_dialog` means the native guarded writer could not parse the
same dialog digest that `agent dialog` observed. The terminal content revision
is recorded in the fingerprint for diagnosis, but a revision change alone does
not reject an answer. Launch, terminal, pane, session, expiry and single-use
checks remain separate guards.

With dev logging (`--dev` or `BUS_LOG=bus=debug`), `bus.dialog.fingerprint`
records the exact parsed observation (`phase="observe"`) and the snapshot
checked under the terminal content lock (`phase="choose"` or `"answer"`).
Match a guarded write's `expected_digest` to the preceding observation's
`dialog_digest` on the same numeric native `pane`. The regular
`bus.dialog.observed` event also gives the terminal and API pane identities.
Use timestamps to select the observation if the same digest was read repeatedly.

Compare these fields:

| Field | What a change establishes |
| --- | --- |
| `dialog_present=false` | No dialog was parseable at the guarded write; the user may have answered it, or a redraw may have temporarily hidden it. |
| `dialog_digest`, `digest_matches` | Whether the current parsed dialog passes the existing guard. |
| `dialog_id`, `text_hash`, `options_hash`, `kind` | Question, command, option content or dialog kind changed. |
| `selected` | Cursor moved to a different option. Selection is excluded from `options_hash`, so this can be distinguished from an option edit. |
| `input_hash` | Focused answer draft or its skip semantics changed. |
| `tabs_hash` | A tab label, answered flag or active tab changed. |
| `hint_hash`, `context_hash` | Key hint or Claude header style context changed. |
| `synchronized_output` | The guarded write ran during an unfinished synchronized terminal redraw; observation snapshots exclude that state. |
| `content_revision` | Terminal updated; if the digest is unchanged, that update did not stale the fingerprint. |

The component hashes are diagnostic, not a second acceptance rule. No question,
command, option label, tab label, draft or submitted answer is logged. A malformed
expected digest supplied directly to the native API is omitted and marked
`expected_digest_valid=false`.

Real Claude 2.1.309 and Codex 0.164.0 PTY captures in
`tests/fixtures/dialog-fingerprint/` exercise the diagnostics. The Codex pair
changes its Working timer into a waiting-for-background-terminal status while
preserving the question and fingerprint. Direction-key captures demonstrate the
existing selection-sensitive guard without asserting that a selection move
caused an incident.

In the retained agent 17039 / room 8259 incident, the 2026-10-22 13:53 UTC
rejection followed a direct user answer by about three seconds. Another rejection
at 14:02 UTC had the same question/labels on two reads but no guarded-write digest
or selection evidence. Those records cannot establish spinner/timer churn as a
cause; this instrumentation preserves the guard while collecting that missing
evidence on recurrence.
