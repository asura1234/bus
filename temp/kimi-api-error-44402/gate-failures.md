# PR #394 runtime failure attribution

## Result

- Pre-existing on base: **1**, intermittent; deterministic base failures: **0**.
- Flaky under load: **12**.
- Introduced by #394: **0**; no production or test fixes were needed.
- Completed **78** isolated invocations: **75 passed, 3 failed**. Every original case ran three times on each snapshot.

## Frozen snapshots and method

- Base: `8b8e7df52e24ab9444798a6e99b9db626dbbf64c` in `/private/tmp/bus-394-base-8b8e7df5`.
- Head: `596ca10c8f75fb6d54dadb0b0a77787a1e8e8bd0` in `/Users/dylanliu/work/bus-kimi-api-error-turn-end`.
- Head initially incorporated #393, then was synced onto master `1cced39d8c9927c153060a5fe459f1daa5e12a39` (including #350’s Codex wire client). All four #394 commits replayed without conflicts or patch changes (`rebase-final.json`). The head side of the matrix was repeated after that sync; base results were retained unchanged. An earlier 39-run head set at `8593f0296` is archived under `attribution/history/`, separate from this table.
- Slot 1: `/tmp/bus-568-build-slot-1`; target `/Users/dylanliu/work/.cargo-targets/bus-shared`. All build/test children used `nice -n 19`, `CARGO_INCREMENTAL=0`, two Cargo jobs and one test process/thread at a time on a 12-core host. Zig builds were limited to two jobs. No machine-wide synthetic load was started.
- Live Bus and secret environment variables were scrubbed. Original assertions and deadlines were preserved. Disk holds interrupted the sequence; all completed runs were retained.
- The 12 `bus` cases used the original gate instrumentation: `RUSTFLAGS="-C instrument-coverage --cfg=coverage --cfg=trybuild_no_target"`, with the coverage child target. The TUI case used an uninstrumented `--features dev-tools --test tui-driver` build. Both TUI CLI binaries were checked for the complete source SHA.
- Built with `cargo test --locked --no-run --message-format=json --bin bus` or `cargo test --locked --no-run --message-format=json --features dev-tools --test tui-driver`. Each selected binary ran as `nice -n 19 EXE --exact TEST --nocapture --test-threads=1`.
- Raw commands, binary hashes, build identity and environment: `attribution/run-isolation.py`, `plan.json`, `environment.json`, and `{base,head}/provenance-{bus,tui-driver}.json`. Build JSON logs retain the `BUS_BUILD_SHA` emitted by build.rs.

## Per-case table

P = pass; F = fail, in run order. Logs are under `/Users/dylanliu/work/bus/temp/kimi-api-error-44402/attribution/{base,head}/ID-rN.log`.

| ID | Exact test | Base | Head | Classification |
|---|---|---|---|---|
| 01 | `server::api::agents::prompt::composer::tests::grok_login_heading_above_composer_padding_receives_no_input` | P / P / P | P / P / P | flaky under load |
| 02 | `server::api::agents::prompt::composer::tests::late_own_paste::initial_observation_inside_a_short_synchronized_frame_waits_then_delivers` | P / P / P | P / P / P | flaky under load |
| 03 | `server::api::agents::prompt::composer::tests::launch_window_only_forwarded_keys_protect_a_try_quote_draft` | P / P / P | P / P / P | flaky under load |
| 04 | `server::api::agents::prompt::grok::tests::grok_unattributed_paste_after_write_submits_once_without_ownership_readback` | P / P / P | P / P / P | flaky under load |
| 05 | `server::terminals::events::tests::grok_captured_composer_prompt::bus_prompt_is_pasted_into_the_synthetic_empty_grok_composer` | P / P / P | P / P / P | flaky under load |
| 06 | `server::terminals::events::tests::grok_captured_composer_prompt::bus_prompt_is_pasted_into_the_captured_grok_layout_when_its_border_has_no_label` | P / P / P | P / P / P | flaky under load |
| 07 | `server::terminals::events::tests::grok_captured_composer_prompt::bus_prompt_is_pasted_into_the_captured_grok_1_0_5_composer_with_model_label_border` | P / P / P | P / P / P | flaky under load |
| 08 | `terminal::runtime::tests::shutdown::close_reaps_installer_after_its_session_leader_exits` | P / P / P | P / P / P | flaky under load |
| 09 | `server::tests::reply_delivery::batched_reply_cycles_hold_fresh_shells_after_reopen` | P / P / P | P / P / P | flaky under load |
| 10 | `server::tests::reply_delivery::provider_fixture_descheduling_does_not_expire_a_committed_submission` | P / P / P | P / P / P | flaky under load |
| 11 | `server::tests::reply_delivery::orchestrator_independent_requests_remain_separate_when_idle_and_after_restart` | P / P / P | P / P / P | flaky under load |
| 12 | `server::tests::reply_delivery::paused_idle_cycle_preserves_reply_across_scheduling_gap_before_freeze` | P / P / P | P / P / P | flaky under load |
| 13 | `fake_grok_compaction_banner_reaches_the_room_notice` | F / P / P | F / F / P | pre-existing on base (intermittent) |

## Pre-existing base failure

`fake_grok_compaction_banner_reaches_the_room_notice` failed once and passed twice on base; it failed twice and passed once on head. The base failure is the same bootstrap failure as the original gate and #393: `tests/tui/driver_test.rs:800` waits 25 seconds for `fake reply: Bus:`, but the fake remains Idle with no current request and the unsubmitted `[Pasted: 3 lines]` chip visible. The test fails before the compaction assertions.

Evidence: `attribution/base/13-r1.log` and `attribution/head/13-r1.log`, `13-r2.log`; preserved driver traces are alongside them in `13-rN-trace/`. Passing controls are base runs 2/3 and head run 3. `attribution/pr-393-context.json` records #393’s independent baseline failures. This is demonstrably intermittent, not a deterministic base failure. The exact internal race is not established by these isolation results and is not repaired in this Kimi PR.

## Scheduling-sensitive failures

Cases 01–12 passed all six current comparison invocations each (3/3 on base and 3/3 on the synced head). They are classified as “flaky under load” using the requested operational criterion: the original broad-gate failures did not recur in these repeated isolated controls. These passing runs do not establish a specific scheduler mechanism.

The earlier head set at `8593f0296` had one additional case-01 callback timeout: `composer_test.rs:240`, the unchanged one-second `completion.recv_timeout(...).unwrap()` after a 200 ms observed-input deadline. It passed before and after that failure (241 ms and 345 ms total invocation durations); the failed invocation took 1206 ms. This earlier failure remains in `attribution/history/head-8593f0296/01-r2.log`; it is not hidden or counted as a passing current-head run.

That earlier set overlapped unrelated real builds elsewhere on the host (case-01 one-minute load averages: 48.6, 39.1 and 109.5 on 12 cores). We added no stress generator. The repeated synced-head set ran after disk reclamation and reduced concurrency in the other room. Per-invocation loads/durations remain in `attribution/results.jsonl` and the archived earlier results; they are context, not a causal measurement.

`attribution/source-blobs.json` proves the eight relevant test/fixture files identical on the frozen snapshots. `subtree-blobs.json` proves the complete `src/terminal`, `src/server/api/agents`, `src/server/terminals`, `src/devtools` and `tests/tui` subtrees identical. Case 01 exercises that unchanged terminal/Grok path, not Kimi settlement. The reply fixtures exercise Claude/Codex; these runs found no introduced failure. No assertions, deadlines or unrelated baseline code were weakened or repaired.

## Historical gate and current validation boundary

The original round-2 gate was FAIL on `03553bec4df6369f99a48408c7729f0d9ae3d1df` against `8b8e7df52e24ab9444798a6e99b9db626dbbf64c`. Its validated artifact is `/Users/dylanliu/work/bus/temp/kimi-api-error-44402/gate-r2/round-2-4301ddbbf04d4d8f9fda98696a98571f.md`; decoded stdout/stderr remain beside this report. Counts were unit 5010/8, integration 313/4, dev-tools 234/1 and Python 1708/1 (passed/failed). The separate Claude-DS import-boundary failure was present on base and was fixed by merged #393; it is not one of the 13 runtime cases above.

The matrix is attribution evidence for the frozen snapshots, not a replacement for a gate. The final gate is run separately after this table is committed on the synced branch. Subsequent changes to the matrix snapshot are limited to this report. Its exact base/head, result and canonical artifact are recorded in the PR and `/Users/dylanliu/work/bus/temp/kimi-api-error-44402/gate-final.md`. The historical failed gate is not relabeled as green.

Raw results and aggregate counts: `attribution/results.jsonl`, `classification.json`. Completed phase coverage profiles were removed after preserving invocation logs and provenance; subsequent builds clean the prior Bus package output. Cleanup actions are recorded in `attribution/cleanup.jsonl`.
