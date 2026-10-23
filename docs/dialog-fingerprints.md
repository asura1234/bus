# Diagnose rejected dialog fingerprints

`stale_or_changed_dialog` means the native guarded writer could not parse the
same completed dialog digest that `agent dialog` observed. Guarded writes wait
up to 40 ms for an unfinished synchronized repaint, releasing the chunk lock so
the PTY reader can finish. If the frame remains unfinished, `dialog_redrawing`
means no keys were sent: observe again and use the fresh fingerprint. A completed
but different dialog is still refused. The terminal content revision is recorded
in the fingerprint for diagnosis, but a revision change alone does not reject an
answer. Launch, terminal, pane, session, expiry and single-use
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
| `synchronized_output` | On `bus.dialog.redrawing`, the write timed out waiting for a completed frame. Component comparisons and observations exclude unfinished frames. |
| `content_revision` | Terminal updated; if the digest is unchanged, that update did not stale the fingerprint. |

The component hashes are diagnostic, not a second acceptance rule. Question,
command, option label, tab label and draft plaintext are omitted, and submitted
answer text is not included in this event. Component hashes are unsalted SHA-256:
someone holding the dev log can confirm guessed short field values, so these
hashes are not a confidentiality guarantee. A malformed expected digest supplied
directly to the native API is omitted and marked `expected_digest_valid=false`.
`digest_matches` is emitted only for a guarded comparison, not an observation.

Real Claude 2.1.309 and Codex 0.164.0 PTY captures in
`tests/fixtures/dialog-fingerprint/` exercise the diagnostics. The Codex pair
changes its Working timer into a waiting-for-background-terminal status while
preserving the question and fingerprint. This is the only captured timer/status
churn coverage: Claude emitted no bytes during its five-second open-question
interval, so its duplicate stationary frame is excluded from the redraw test.
Direction-key captures from both providers demonstrate the existing
selection-sensitive guard without asserting that a selection move caused an
incident.

In the retained agent 17039 / room 8259 incident, the 2026-10-22 13:53 UTC
rejection followed a direct user answer by about three seconds. Another rejection
at 14:02 UTC had the same question/labels on two reads but no guarded-write digest
or selection evidence. Those records cannot establish spinner/timer churn as a
cause. A separate regression proves that parsing between chunks of an unfinished
synchronized repaint can reject the same completed dialog. The guarded writers
now wait for a coherent frame and report retryable redraws; this mechanism is not
proven to explain the retained incident. Diagnostics preserve the exact-digest
guard and collect the missing evidence on recurrence.
