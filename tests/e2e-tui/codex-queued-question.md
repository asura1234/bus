# Codex queued question

## Run contract

Use the e2e-test skill and the branch's `./run tui`, built with `CARGO_INCREMENTAL=0` and its
own target. Run CQ-01 in a fresh scratch session at **160x45**, default kitty graphics. No SKIP is
allowed. Remove `BUS_DATA_DIR`, `BUS_SESSION_ID` and every `HERDR_*` variable from the
environment of every driver invocation. Never drive the live Bus or run a real provider.

The fake Codex `fake:permission question-warning` directive reproduces what real Codex 0.162
draws after `request_user_input_async` while its turn keeps working: it switches to the alternate
screen, shows `• Working (… esc to interrupt)`, the `• Queued follow-up inputs` banner with
`? 1 question` and `⇧← to answer`, an empty composer, and a footer whose `? for shortcuts` row
also carries a right-aligned `⚠ 1 warning · f2 to view`. Shift+Left opens the question
`Which design should the worker use?` with options `1. Alpha` and `2. Beta`; Enter answers it.

## Fixture setup (allowed scratch CLI)

`T` means `./run tui` with the case's `--name` on every call. Start the session and record
`status`. `WORK` is the session's scratch work directory.

```text
T bus room create question-lab
T bus agent add --room question-lab --name worker --provider codex --pwd WORK --consent-hooks
```

Wait on `bus state` until the worker and the auto-created `permission handler` are idle. Bus
routes every blocked episode to that handler: for this question its fake Claude runs the real
`bus agent dialog` (which opens the queued question) and posts a `Routed` notice to the room
without answering. CLI calls beyond setup are limited to the read-only `state`,
`history --room question-lab`, `agent read worker ...` and `agent dialog worker`, plus the one
`agent choose` in step 4, which is the feature under test.

### CQ-01 — Collapsed question beside a footer warning is readable and answerable

1. Complete setup. Click `room:question-lab`, press `ctrl+p`, click `recipient:worker`, click
   `composer`, paste `fake:permission question-warning`, press `enter` (one driver call each).
   Wait up to 30s until `T bus history --room question-lab` holds the handler's notice about the
   worker (`worker (agent N): Routed; ...; Inspect: bus agent dialog N`). The scratch server log
   (copied into the run folder at stop, or read under the session's data dir) has a
   `bus.dialog.observed` line for the worker with `status=Working` and
   `listed_dialog_id=Some("codex:pending_question")`: Bus saw the collapsed question while the
   turn was working, with the warning on the footer row. The handler's private
   `fake-permission-cli.ndjson` shows its `agent dialog` returned the `Which design should the
   worker use?` choice with a fingerprint. Preserve both.
2. Run `T bus agent read worker --source recent --lines 120` while the worker is still not idle.
   It succeeds (no `error`), `capture.exhausted` is `false`, `capture.note` says only the
   visible screen was read while the worker is busy and to retry when idle, and `text` holds the
   question `Which design should the worker use?`.
3. Run `T bus agent dialog worker`: `dialog` is non-null, its text is
   `Which design should the worker use?`, its options are `Alpha` and `Beta` (1 selected), and
   `fingerprint` is non-null. The worker's `dialog_notice` in `T bus state` is non-null. Take and
   open a screenshot of the room.
4. Run `T bus agent choose worker --option 2 --fingerprint F` with that fingerprint. The
   outcome is `closed`. Within 30s `T bus agent dialog worker` returns `dialog: null`, the
   worker returns to `idle`, and the room history has the worker's
   `fake reply: fake:permission question-warning`. Stop the session.
