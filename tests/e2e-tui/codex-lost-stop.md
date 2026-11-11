# Codex turn completion without a Stop hook

## Run contract

Use the e2e-test skill and the branch's `./run tui`, built with `CARGO_INCREMENTAL=0` and its
own target. Run CLS-01 in a fresh scratch session at **160x45**, default kitty graphics. No SKIP
is allowed. Remove `BUS_DATA_DIR`, `BUS_SESSION_ID` and every `HERDR_*` variable from the
environment of every driver invocation. Never drive the live Bus or run a real provider.

The fake Codex writes a rollout as Codex 0.163.0 does: each turn's `task_started`, its
`UserMessage` and its `task_complete` with `last_agent_message`. A prompt containing
`fake:lose-cwd` plays a worker that removes its own working directory mid-turn (live,
2026-10-20): the turn's submit hook runs, but from its `Stop` on the fake runs no hooks at all,
while its rollout still records every native answer. Bus must use the attributed
rollout end to settle each request as `ended`, with no stored reply, rather than
closing the messages as `reply_not_captured`. The exact answer remains native evidence.

## Fixture setup (allowed scratch CLI)

`T` means `./run tui` with the case's `--name` on every call. Start the session and record
`status`. `WORK` is the session's scratch work directory.

```text
T bus room create stop-lab
T bus agent add --room stop-lab --name worker --provider codex --pwd WORK --consent-hooks
```

Wait on `bus state` until the worker is idle. CLI calls beyond setup are limited to the
read-only `state`, `history --room stop-lab`, `message status` and `agent read worker ...`.

### CLS-01 — Bound and unbound Codex turns settle without a Stop or a stored reply

1. Complete setup. Click `room:stop-lab`, press `ctrl+p`, click `recipient:worker`, click
   `composer`, paste `fake:lose-cwd merge the PR`, press `enter` (one driver call each). Within
   30s the native pane shows `fake reply: fake:lose-cwd merge the PR`
   (corroborate with `agent read worker --source recent`).
   `T bus history --room stop-lab` lists the original prompt exactly once, with no
   stored reply. Pass that history entry's `prompt.id` to `T bus message status MESSAGE_ID`: the
   status is `complete:true`; the worker's `requests[]` entry has `stage: "ended"`,
   `reply:null` and `reason: null`. In `T bus state` the
   worker has no `actionable_error` and is `idle`. Take and open a screenshot of the room.
2. Click `composer`, paste `fake:work 3 copy the evidence`, press `enter`. This turn runs no
   hooks at all, so Bus never sees it start; `fake:work 3` keeps it working for 3 seconds, as a
   real turn takes, so Bus sees the agent Working and then Idle. Within 30s its native pane shows
   `fake reply: fake:work 3 copy the evidence`.
   `T bus history --room stop-lab` lists each original prompt exactly once and
   both requests have no stored reply. Check both `prompt.id` values with `T bus message status MESSAGE_ID`:
   the second request is complete with `stage: "ended"`, `reply:null` and `reason: null`, and neither request has
   `reason: "reply_not_captured"`. Take and open a screenshot of the room. Stop the session.
