# Reply delivery regression inventory and integration suite

Inventory baseline: `6f27346a` (master on 2026-10-19). Sources: every reachable Bus messaging fix since the verified messaging introduction `c6a1b245`, the complete 107-entry fork PR list, and the relevant diffs/added regression tests. GitHub issues are disabled for this fork (`gh issue list --state all`); there is no issue list to inventory. Inherited Herdr terminal/SSH/graphics bugs, presentation-only copy/spacing fixes, and external Feishu notification delivery are outside agent-reply delivery. The suite is rebased onto `a6e6a04d6904bf50f06315f9bc3d21a2893d6769`, where #107 is merged; its two regression cases now run in ordinary gates.

The table consolidates repeated fixes by failure scenario. The new suite is `server::tests::reply_delivery::`; short names below identify those tests. Retained guards name existing narrower tests, not claims of new end-to-end or historical-red proof. All of them remain in the full local gates. Historical file moves from `src/bus` to `src/messaging` were followed while inspecting the original diffs.

| Historical source | Failure scenario | Guard |
| --- | --- | --- |
| [f12280d8 / d3008f80](https://github.com/asura1234/bus/commit/f12280d8) | Prompt correlation lost after whitespace trimming or a stale callback matched an identical prompt. | matrix, image/paste; retained queue late-identical-final guard |
| [3fd44832](https://github.com/asura1234/bus/commit/3fd44832) | A newly interactive provider remained behind the manual setup gate; every message stayed queued. | retained resume interactive-ready + scheduling first-Codex guards |
| [0dcc4bd2 / 44e94f38](https://github.com/asura1234/bus/commit/0dcc4bd2) | Cursor injected user rows, omitted commentary suffixes and advanced transcript records prevented final recovery. | Cursor advanced-transcript integration; retained Cursor final-reply guards |
| [3c96cbf8](https://github.com/asura1234/bus/commit/3c96cbf8) | A continued request lost ownership or could not be explicitly recovered. | held wake/restart; retained recovery guards |
| [489529a9 / c5d7807e](https://github.com/asura1234/bus/commit/489529a9) | Claude atomic long-paste or lifted image placeholders no longer matched the submitted prompt. | image/paste, coalesced sends |
| [a6e54461](https://github.com/asura1234/bus/commit/a6e54461) | A shell from an older turn, including a hung find, held every later reply indefinitely. | five held turns, soak; retained inherited/unknown-age guards |
| [44d199a7](https://github.com/asura1234/bus/commit/44d199a7) | A later task-notification or separately typed turn cleared the captured reply; batched callback ordering applied starts before earlier Stops. | unrelated turn and duplicate Stop, five held turns |
| [0ce17ba7](https://github.com/asura1234/bus/commit/0ce17ba7) | A prompt folded into an autonomous wake had no trusted submit hook, wedged MASTER, and broke queued delivery after restart. Resume hooks could still point at an older binary. | held wake/restart, process restart; retained cold-resume/rebind guards |
| [bc8d6000](https://github.com/asura1234/bus/commit/bc8d6000) | Ended turns with no Stop never released their requests, including joined Codex messages. | Codex rollout failure; retained ended-turn settlement guards |
| [6060e262](https://github.com/asura1234/bus/commit/6060e262) | Frequent read-only status polling starved message delivery, or stalled queued work ran later after the waiter had given up. | retained scheduling status-poll and durable queued-expiry guards |
| [bd439df3](https://github.com/asura1234/bus/commit/bd439df3) | Busy/unbound Claude follow-ups waited for idle; coalesced atomic pastes with adjacent input did not complete every joined message. | coalesced sends, held MASTER follow-up; retained adjacent-paste guard |
| [#7 / 11f72cb4; #11 / 4a64a33f](https://github.com/asura1234/bus/pull/7) | Transient state/history persistence failure lost delivery responsibility or acknowledged callbacks before durability. Review fixes additionally fenced callback identity, report routing and stalled queue recovery. | failed-persistence/replay; retained callback-batch/history-append/binding/recovery guards |
| [#11 / 4a64a33f](https://github.com/asura1234/bus/pull/11) | Separate orchestrator turns with identical text were deduplicated against an earlier report, or a final already sent to the developer was posted twice. Turn boundaries needed to survive restart. | identical autonomous orchestrator reports; retained same-turn explicit-report dedupe and persisted turn-boundary guards |
| [#11 / 4a64a33f](https://github.com/asura1234/bus/pull/11) | An idle Codex approval looked finished, letting the request or queue expire; a rejected steering write cleared the lead's turn-end evidence. | startup dialog; retained idle-approval and rejected-steering coordinator/model guards |
| [#11 / 4a64a33f](https://github.com/asura1234/bus/pull/11) | A developer's literal Cursor task-notification text was mistaken for an autonomous notification, dropping its final; trailing whitespace beside a Claude image defeated matching. | image/paste and provider matrix; retained literal-Cursor-notice and whitespace-sensitive image callback guards |
| [#15 / cae6359e](https://github.com/asura1234/bus/pull/15) | First Enter could be swallowed while slow full-state saves kept the send pending. | retained client send-first-enter guard; process round trip checks CLI acceptance, not the keyboard layer |
| [#16 / 2b2729d2](https://github.com/asura1234/bus/pull/16) | Claude lifted a quoted image into a chip: the native composer observer never pressed Enter and all later work queued behind it. Refused pastes needed withdrawal before retry. | image/paste through real guarded native writer; retained prompt-not-shown withdrawal guard |
| [#21 / 5453529d](https://github.com/asura1234/bus/pull/21) | Fresh Codex looked idle under its startup update chooser; the prompt Enter selected Update and the request became submission_unconfirmed. | startup dialog, retained self-update/relaunch guard |
| [#23 / 612c15a1](https://github.com/asura1234/bus/pull/23) | An exited provider reported a late session and made the wrong conversation resumable. | retained native terminal session-admission tests; process restart exercises session restoration |
| [#28 / c7e56569](https://github.com/asura1234/bus/pull/28) | A nested claude -p or codex exec inherited Bus hooks and replaced the outer session or marked it unavailable. | coordinator-only flagged-record integration; retained hook ancestry admission guards |
| [#43 / 29debd16](https://github.com/asura1234/bus/pull/43) | Async wait stopped on transient transport errors; async send emitted two JSON documents or lost its receipt. | real socket EAGAIN; retained CLI async wait/output integration guards |
| [#44 / 87c75706](https://github.com/asura1234/bus/pull/44) | Own-turn shells caused premature completion, while Codex capacity errors with no Stop became unexplained abandonment; joined messages lost the same reason. | five held turns, held follow-up, Codex rollout failure; retained joined-error guard |
| [#45 / 28ecf40d; #67 / d95c6931; #70 / b2a41686](https://github.com/asura1234/bus/pull/45) | Dialog delivery/queued native questions were lost or stale blocked-help ownership prevented the eventual request reply. | startup dialog; retained Codex question-answer and settlement guards |
| [#49 / a5a95935](https://github.com/asura1234/bus/pull/49) | Claude StopFailure API errors were reported without provider_error and could leave callers waiting without an actionable reason. | retained Claude provider-error settlement test |
| [#50 / 7619acaa](https://github.com/asura1234/bus/pull/50) | Autonomous worker/background reports were routed into MASTER rather than their work room. | autonomous worker + waiter; retained work-room report routing guard |
| [#52 / f1a0cbc8](https://github.com/asura1234/bus/pull/52) | An idle-looking orchestrator held for background shells never received a non-queued MASTER/work-room follow-up; queued work could expire across wake gaps. | held MASTER cross-room follow-up, held wake plus queue |
| [#56 / 255fe33a](https://github.com/asura1234/bus/pull/56) | Archiving settled history could lose replies/unread delivery during load or race history reads. | reopen after exact history assertions; retained archive/readers/append recovery guards |
| [#60 / b44cb047](https://github.com/asura1234/bus/pull/60) | Shell-held requests and their queues falsely became transcript_unmatched; unreported follow-ups did not settle at the bounded shell cap. | five held turns, held follow-up, soak; retained shell-cap/stall guards |
| [#82 / c7338c43](https://github.com/asura1234/bus/pull/82) | A shell wake continuation cleared awaiting_background then ended without Stop: Bus abandoned its already captured interim as reply_not_captured, including joined messages and restart. Bus-only send/wait shells also held replies unnecessarily. | held no-Stop wake/restart + queue, direct controls |
| [#92 / 79d95907](https://github.com/asura1234/bus/pull/92) | A worker reply stayed only in room history when its original orchestrator waiter exited; routing/restart/ack could duplicate or drop terminal delivery. | worker reply wakes orchestrator once, retained waiter/seen routing guards |
| [#96 / ec1a73e7](https://github.com/asura1234/bus/pull/96) | Five separately answered turns shared the last answer; inherited shell wakes could overwrite a newer request answer. Folded turns still need one shared answer. | five held turns, coalesced sends, held cross-room follow-up; retained inherited-wake guard |
| [#98 / dae6fa05](https://github.com/asura1234/bus/pull/98) | Autonomous finals, no-reply outcomes and idle/exit events never woke the orchestrator; a live waiter could suppress unrelated outcomes. | autonomous worker with live waiter, worker wake/reopen; retained event/outcome overlap guards |
| [#103 / 6438f2ef](https://github.com/asura1234/bus/pull/103) | Quoted direct Bus executable paths and control verbs beyond send/wait were misclassified as work shells and held captured replies; developer and agent senders appeared different. | direct-control matrix for both authors in MASTER |
| [#106 / 55b78b17](https://github.com/asura1234/bus/pull/106) | Slow server polling hit EAGAIN then left transport diagnostics in composer and stalled delivery. Requests must retain ownership while polling recovers. | real socket EAGAIN with active spooled final; retained poll retry/budget guards |
| [#107 (stall-cx, open at inventory time)](https://github.com/asura1234/bus/pull/107) | A Stop with cron/non-shell background work, including alongside live shells, discarded captured reply text before the bounded hold; Working/Idle/restart repeatedly rearmed transcript_unmatched alerts for one request. | two enabled regression tests plus the per-cycle stall soak phase; all reproduced RED before #107 |
| this PR (image beside collapsed paste) | Claude showed text long enough to collapse, sent with an image, as `[Image #1][Pasted text #2 +3 lines]`; the composer observer withdrew it as never shown every 20 s and every message queued behind it stayed queued (live 2026-10-21). Codex 0.163.0 shows a message that is only an image as `[Image #1]`, which hit the same withdrawal. | image chip beside a collapsed paste, from a captured Claude 2.1.306 composer; Codex image-only message, from a captured Codex 0.163.0 composer; retained composer collapsed-paste count guards |

## Test boundary

The harness opens a real coordinator on a unique private scratch directory, seeds owned native terminals, sends through `message.send`, and drives full `Worker::tick` calls. On Unix, every native delivery uses the production API client and socket connection handler against a real server `App` and terminal writer; other platforms retain the direct App boundary. Native guarded prompt APIs, composer observation and Enter submission execute unchanged. Hooks are durably spooled and parsed by the production Claude/Codex adapters; replies are checked through request completion, `room.history`, independent per-room publication counters and mirrored-notice rejection. Completed reply groups contribute one publication per source room; agent-authored input contributes a separate publication. History-key uniqueness alone is not an exactly-once oracle. No matcher, request-completion or reply-posting function is mocked. The cross-room orchestrator follow-up is created by real blocked-worker dialog polling, because user/CLI recipient selectors are scoped to agents physically in the selected room. Only provider terminal output, status and hooks are scripted. The architecture checker has exact test-only exceptions for the two integration fixture paths; production and neighboring test paths retain the original import rules.

Claude uses `prompt_id`, lifted image chips, atomic paste frames and `background_tasks`/`session_crons`. Codex uses `turn_id`, an interactive `transcript_path`, and JSONL `event_msg/task_complete` provider errors. These are different contracts: Claude finals come from Stop hooks, not a standalone transcript reader. The image composer uses the independently captured `src/server/api/agents/tests/fixtures/claude_image_paste.txt`, never the production image-lifting rule. The fixture also records their JSONL shapes; recording a transcript is not evidence that Claude reads it.

A captured Stop answer completes its requests immediately, independently of
background work and native Working status. A later changed final on the same
attested launch/session/turn remains a **Late reply** follow-up; the original
completed result stays unchanged.
The same rule covers Codex Stops without a background-task census. Identical text is
deduplicated across envelopes and restarts, while successive changed finals remain
deliverable, including after abandonment. Coalesced and steered members use their
group lead's attested submission boundary and turn; an orchestrator-authored member
establishes who awaits the late answer. New input binding to an already consumed
turn cannot suppress the old request's follow-up. Fresh autonomous turns retain
their generic **Turn final** notice without inventing a request attribution.
If a later request adopts the same turn and is abandoned, replaying that earlier
autonomous result neither republishes it nor relabels it as the later request's
late reply. Autonomous turns still publish only once.
Coordinator regressions cover timeout/ordinary completion, restart, newer bound
and unbound input, developer-led groups, both Stop shapes, and repeated/foreign
callbacks.

Deadline scenarios age persisted clock fields then run the real coordinator. This avoids sleeping for settlement deadlines; it does not replace the live socket deadline or soak wall clock. The EAGAIN test uses the production native API client, real local sockets, production socket connection handler and App dispatcher. It pauses server dispatch and waits for the real three-second receive timeout (WouldBlock/EAGAIN on macOS, TimedOut on other kernels), then recovers without resubmitting the prompt.

`reply_delivery_survives_actual_server_restart_mid_request` runs in the dev-tools `tui-driver` integration target. It starts its own PTY Bus with fake providers, pauses only its coordinator after a trusted start, waits for a durable Stop, shuts down the actual native server, kills that scratch coordinator and starts a new server/coordinator on the same scratch data. It covers Claude in MASTER and Codex in a separate work room. The default first work room is an observer, so neither reply source is visible across restart; publication counters in both source rooms and standalone notices detect duplicate or wrong-room posting, and repeated status/history reads must leave counters unchanged. No desktop automation or installed provider is used; fake provider/effect executables, HOME, project and data are private to that test.

## Run

Use the isolated worktree's one target directory; none of these commands touches the running Bus or the main checkout's `target/debug`. Acquire a room build slot for the run and release it on exit.

```sh
cd /Users/dylanliu/work/bus-test-reply-delivery
export CARGO_TARGET_DIR="$PWD/target"
export CARGO_LLVM_COV_TARGET_DIR="$CARGO_TARGET_DIR"
with_build_slot() (
  slot=
  until [ -n "$slot" ]; do
    if mkdir /tmp/bus-877-build-slot-1 2>/dev/null; then slot=/tmp/bus-877-build-slot-1
    elif mkdir /tmp/bus-877-build-slot-2 2>/dev/null; then slot=/tmp/bus-877-build-slot-2
    else sleep 60; fi
  done
  trap 'rmdir "$slot"' EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  "$@"
)
with_build_slot cargo test --locked --bin bus server::tests::reply_delivery
with_build_slot cargo test --locked --features dev-tools --test tui-driver reply_delivery_survives_actual_server_restart_mid_request -- --exact --nocapture

# Separate ignored soak: short default, one wall-clock minute.
with_build_slot cargo test --locked --bin bus server::tests::reply_delivery::soak_many_agents_concurrent_messages_and_background_shells -- --exact --ignored --nocapture
# Long mode: 30 wall-clock minutes (any positive value up to 1440).
BUS_REPLY_SOAK_MINUTES=30 with_build_slot cargo test --locked --bin bus server::tests::reply_delivery::soak_many_agents_concurrent_messages_and_background_shells -- --exact --ignored --nocapture

# #107 regressions are enabled by default; narrow reruns need no --ignored.
with_build_slot cargo test --locked --bin bus server::tests::reply_delivery::background_pending_cannot_discard_captured_reply_or_block_queue -- --exact --nocapture
with_build_slot cargo test --locked --bin bus server::tests::reply_delivery::transcript_unmatched_diagnostic_is_not_rearmed_by_status_flapping_or_restart -- --exact --nocapture
```

The soak uses twelve concurrent producer threads and keeps twelve agents active with first and queue-only messages. Each cycle spools all agents' starts, ticks once, spools all Stops, and ticks once. Reply text differs from the prompt. Every Claude first and queued Stop must complete despite running shells. Fresh own-shell ids accompany a persistent shell report. After each coordinator reopen, explicit provider startup hooks establish known shell ownership again; unknown shells after restart are intentionally treated as inherited in production.

Every cycle also positively triggers an alert: it ages a new unsettled request, flaps Working/Idle and reopens, requires exactly one durable diagnostic marker for that message, then delivers the late reply. The delivery phase rejects unexpected alerts. This catches repeated-alert behavior while an unsettled request exists. The stall phase reproduced RED on pre-#107 master and passed on pinned #107 production head `e9020a8e`; it now runs in the single official soak target against merged master.

The soak stops admitting cycles after N wall-clock minutes and drains the admitted cycle. Each cycle has 25 requests, all of which must complete with distinct expected replies, exactly one publication per reply group/source room, and one deliberate stall diagnostic with no repeats. The printed elapsed time includes the drained final cycle. `batched_reply_cycles_hold_fresh_shells_after_reopen` runs two complete shared cycles (50 requests, two single stall diagnostics) in default gates, so every phase runs even when the timed target is not selected. Only the timed soak is ignored; the two #107 regression tests are enabled.

## Per-test contract

All names below are under `server::tests::reply_delivery::` unless specified.

| Test | Failure caught |
| --- | --- |
| `open_messaging_cross_room_session_delivery_has_its_own_reply` | MASTER/work peers receive attributed messages across rooms, bind the exact native payload after restart and return their own reply in the conversation room. |
| `open_messaging_pending_cross_room_messages_do_not_coalesce` | An unavailable recipient's pending messages from different rooms keep separate turns and replies. |
| `worker_message_typed_into_orchestrator_names_its_worker_and_room` | The exact `name (room R):` payload survives restart and matches the provider callback; IDs appear only for numeric or historical names. |
| `numbered_native_draft_edits_never_publish_blocked_or_resolved_notices` | Editing a numbered Claude native composer must not become a series of false dialog IDs and Blocked/Resolved notices, in MASTER or a work room, including restart. |
| `numbered_codex_native_draft_edits_never_publish_blocked_notices` | Editing a numbered Codex composer above its model footer must not request dialog attention in MASTER or a work room. Adopted red-first from the Claude review. |
| `dialog_episodes_publish_once_despite_detection_flaps_and_restart` | Two real episodes with the same menu each publish one Blocked and one Resolved; synchronized redraws with missing detection, selection changes, status flapping and coordinator reload cannot repeat or prematurely resolve them. |
| `captured_reply_reaches_its_source_room_once_for_both_providers_and_authors` | Claude, Codex and Cursor; MASTER/work; identical developer/agent messages; completion, publication counters and no mirrored notice after restart. |
| `cursor_injected_user_rows_and_advanced_transcript_keep_the_owned_final` | Injected user rows, omitted commentary suffixes and a newer transcript turn cannot replace the owned Cursor final (0dcc4bd2/44e94f38). |
| `independently_answered_turns_keep_five_distinct_replies_while_shells_live` | Five answered turns with shells persisting across turns must keep five distinct answers (#96). |
| `queued_worker_notice_after_held_master_turn_reaches_work_room_and_master_once` | An automatically routed blocked-worker notice takes its own turn after the captured MASTER Stop answer; each answer publishes once in its source room while shells remain running. |
| `captured_stop_survives_a_no_stop_wake_and_restart_before_the_next_request` | The immediate captured answer survives a no-Stop wake and restart; queue-only work then gets its own answer (#82). |
| `coalesced_sends_share_one_answer_and_one_history_publication` | Three queued messages become one native paste and one shared reply; atomic Claude paste, Codex and unread dedupe. |
| `direct_bus_control_shells_publish_equally_for_developer_and_agent` | Captured answers publish for both senders while background control shells remain running (#103). |
| `captured_reply_survives_unrelated_later_turn_and_duplicate_stop` | An autonomous later turn cannot replace a captured answer; replay cannot double-post. |
| `flagged_nested_provider_callbacks_do_not_steal_active_request_identity` | Coordinator-only: records already flagged nested cannot replace the outer interactive session. Hook ancestry classification is covered by retained hook-side tests, not this fixture (#28). |
| `claude_image_chip_and_long_paste_bind_to_the_delivered_request` | A captured real Claude screen independently drives the guarded writer image chip submission, then the atomic pasted submit hook binds (#16 and early matching fixes). |
| `codex_startup_dialog_keeps_requests_queued_until_composer_recovers` | No prompt or Enter reaches the update chooser; pending work replies after readiness (#21). |
| `autonomous_worker_final_routes_even_while_original_reply_has_a_waiter` | A live waiter, refreshed immediately before the asserting tick, cannot suppress an unrelated worker outcome (#98). |
| `identical_autonomous_orchestrator_reports_are_distinct_across_turns` | Identical text in distinct background turns publishes twice in the work room; replay of the same turn publishes once (#11/#50). |
| `codex_rollout_provider_error_is_explicit_and_does_not_poison_next_reply` | A no-Stop Codex task_complete error is explicit abandonment, and the next queued request still replies (#44). |
| `failed_reply_persistence_replays_spool_after_restart_without_duplication` | Failure is injected after the Idle poll, into the callback batch save (explicitly not poll); the Stop remains unacknowledged for replay; final appears once (#7/#11). |
| `worker_reply_wakes_orchestrator_once_without_waiter_and_survives_reopen` | A dead/absent waiter does not lose the worker result; persisted routing does not re-submit it (#92). |
| `socket_eagain_during_active_reply_preserves_ownership_and_recovers_once` | A real three-second kernel/API receive timeout leaves the active request and captured callback owned; recovery delivers once and native `typed` count stays one (#106). |
| `background_pending_cannot_discard_captured_reply_or_block_queue` | Captured Stop with cron or non-shell tasks, with/without a live shell, must deliver and drain queue in MASTER and work room. Enabled after merged stall-cx #107. |
| `transcript_unmatched_diagnostic_is_not_rearmed_by_status_flapping_or_restart` | One unmatched request must generate one alert despite Working/Idle flapping and reload. Enabled after merged stall-cx #107. |
| `batched_reply_cycles_hold_fresh_shells_after_reopen` | Two shared load cycles batch hooks, require immediate Stop completion with live shells after reopen, compare distinct replies/publications and trigger one stalled-request alert per cycle. Runs in ordinary gates. |
| `reply_contract_rejects_second_visible_publication` | Negative control: a real additional MASTER publication must be rejected even when the request history map still has one row. RED with the old assertion helper; not a claim of a new product defect. |
| `soak_many_agents_concurrent_messages_and_background_shells` | Every admitted request completes with the right reply under 12-agent sustained load; each cycle also ages a stalled request, flaps/restarts and requires one alert before late delivery. Separate ignored timed target. |
| `reply_delivery_survives_actual_server_restart_mid_request` (dev-tools `tui-driver`) | Actual OS server/coordinator restart with a durable unconsumed final, in both provider/source-room configurations. |

## Evidence

Exact tested heads, historical-red output, measured runtimes, gate artifacts and the Claude review are recorded in the PR validation report. Compilation errors and fixture setup errors are not counted as historical RED evidence. No delivery/matching production implementation is changed by this suite.

## Orchestrator inbox admission during background waits

Captured Stop answers complete immediately. Only a textless background pause
needs inbox admission: an idle orchestrator can close it as
`background_wait_superseded` and submit queued input under its own identity.
A live generation or native dialog is not an admission boundary. The grace for
autonomous starts protects native status lag.

Actual blocked-worker notices, identified by their persisted dialog-routing request
IDs, take priority over ordinary orchestrator inbox messages and get separate
turns. Other senders cannot acquire priority by imitating notice text. Direct
human-typed Claude turns are identified from matching session/turn transcript
metadata and do not become continuations of an unrelated Bus request. Bus-submitted
input and already-bound folded turns retain ownership.

The enabled `orchestrator_inbox_*` regressions prove early cross-room admission,
source-room attribution and replay/restart safety, real dialog priority while a
live turn stays uninterrupted, direct human input followed by a no-Stop interrupt,
and a textless background Stop. The original four reproduced RED on `4e30f80e` before the fix. A folded-steering guard requires a matching hook or transcript before sharing an answer. Unclaimed input surfaces its uncertain outcome at the idle boundary, allowing the next room to proceed.

Both the ordinary two-cycle guard and the official timed soak now run an additional
orchestrator burst per cycle: 28 queued messages alternate MASTER and the work room,
a real worker dialog must take priority, and each answered turn starts another
long background shell. The burst drains while every captured Stop answer publishes immediately,
checks distinct replies and visible publication counts, and reopens the coordinator
four times with explicit provider startup hooks. Each burst proves 30 additional
requests (initial answer, blocked notice, and 28 queued inputs). The original twelve-agent
load and stall-alert phases retain their independent counts and assertions.

The timed soak also asserts one ordinary Bus-authored idle-worker notice per aged
stall cycle, separate from its deliberate MASTER stall alert. Its unread oracle
counts those notices as well as the 25 load requests; the independent publication
oracle and negative duplicate-publication control remain active. A diagnostic
one-cycle replay proved the former 26-versus-25 count was an ordinary 11-idle-worker
notice, rather than an extra reply.
