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

Wait on `bus state` until the worker is idle. CLI calls beyond setup are limited to the
read-only `state`, `history --room question-lab`, `agent read worker ...` and
`agent dialog worker`, plus the one `agent choose` in step 4, which is the feature under test.

### CQ-01 — Collapsed question beside a footer warning is readable and answerable

1. Complete setup. Click `room:question-lab`, press `ctrl+p`, click `recipient:worker`, click
   `composer`, paste `fake:permission question-warning`, press `enter` (one driver call each).
   Wait until `T bus agent read worker --source visible` shows `? 1 question`, `⇧← to answer`
   and `⚠ 1 warning · f2 to view`, and state shows the worker `working`. Capture both.
2. Run `T bus agent read worker --source recent --lines 120`. It succeeds (no `error`), its
   `text` contains `? 1 question`, `capture.exhausted` is `false`, and `capture.note` says only
   the visible screen was read while the worker is working and to retry when idle.
3. Wait up to 30s until the worker's `dialog_notice` in `T bus state` is non-null and
   `T bus history --room question-lab` holds a `Blocked, needs help` notice about the worker.
   Then run `T bus agent dialog worker`: `dialog` is non-null, its text is
   `Which design should the worker use?`, its options are `Alpha` and `Beta` (1 selected),
   and `fingerprint` is non-null. Take and open a screenshot of the room.
4. Run `T bus agent choose worker --option 2 --fingerprint F` with that fingerprint. The
   outcome is `closed`. Within 30s `T bus agent dialog worker` returns `dialog: null`, the
   worker returns to `idle`, and the room history has the worker's
   `fake reply: fake:permission question-warning`. Stop the session.
