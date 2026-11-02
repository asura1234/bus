# Review Round 2 — 代码审查

**分支**：fix/codex-rollout-final-answer @ ad9dc509e1a63a8ecd99cb4136d01e54a1ce9266
**基线**：origin/master @ 306b177332aaefd3addfbbb6c6f1cafb1b3b8a7c
**计划（如有）**：无
**锁定目标**：Recover the exact final answer of the matched Codex 0.164.0 rollout turn when missing launch-directory hooks cannot report it, and show that agent as Unavailable with the cause only in the client log; prove both with recorded-fixture TDD, a final-head full gate and private real-Codex acceptance.
**审查者**：codexrevf
**姿态**：standard
**日期**：2026-11-02

## 前轮问题核销

| # | 来源 | 问题 | 核销状态 | 证据 / 去向 |
|---|------|------|----------|-------------|
| 1 | R1-1 | Missing-cwd availability suppresses interruption and expires a busy queue | satisfactory | `src/messaging/model/status.rs:70` retains activity separately; `src/messaging/coordinator/control/agents.rs:321` and `:404` use it for dispatch/targeting; `src/messaging/model/requests.rs:221` preserves busy queues; `src/messaging/coordinator/control/inspect.rs:268` exports activity and `src/cli/control/interrupt.rs:140` uses it with the legacy status fallback. All three adopted reviewer cases, the producer test, and the CLI waiter test pass in `evidence/review-repairs-green.log`. |

> 总结：R1-1 satisfactory；前轮问题已在根因上核销。

## 新问题与建议

无。

## 同步清单（CONSISTENCY drift，非阻塞）

无。

## 本轮探索区域

- Read 的源码文件：Incremental review of both repair commits `a4b19b4081e295b9245ca2a2e76978740c093d46` and `ad9dc509e1a63a8ecd99cb4136d01e54a1ce9266`, covering all 13 delta files. Reconciled this lane's R1-1 and read the unified triage ledger; no full branch rescan or fresh-eyes fan-out. New fixture and regressions serve the unchanged locked goal and originating brief.
- Activity repair：The common activity projection keeps native status plus existing dialog/missing-binary rules. Shown status still overrides cwd loss to Unavailable. Both interrupt consumers, busy-queue expiry, the control producer, and the CLI waiter now agree on activity. Legacy servers without the additive field still use status. The adopted cases exercise native Working and bound Idle interruption, one Esc, no premature settlement, shown Unavailable remaining visible, and native Working/Blocked queue retention. Existing stop/unavailable checks are in the selected run. The directory-reappearance test still checks that clearing the cwd error restores the native shown state, and the log-only warning tests pass.
- Parser delta：Native completed AgentMessage body now precedes the rendered response for the exact matched turn before task completion. Explicit empty native text remains a final. Response-only text support, phase/completion/turn checks, legacy final text and provider-error precedence remain intact; citation-only rendered response metadata yields empty text. The changed expected bodies are justified by the recorded Stop control rather than by the implementation under review.
- Fixture control：`evidence/fixture-control-audit.json` verifies the new four-line fixture is byte-identical to original rollout source lines 2, 205, 206 and 209; original SHA-256 `b325dc3991004a40a65e552a15d9be8ea2c01cb17a1abd18bc0226e6a26feef9`. The saved history's request 36975 is stage replied with empty reply text and the same provider turn ID. Stop-parity, response-only citation metadata, the four recorded finals, exact text-part preservation and same-turn/completion-boundary cases all pass in the scoped log. The parser root is the unified ledger's separate canonical repair; it is not added as another prior item for this lane.
- 运行的测试：Reviewer ran no cargo build/test, gate or live provider session and made no source edits. Reviewed the author's exact-head scoped run, confirmed by Bus message 37527 and `evidence/author-triage.md`: `cargo nextest run --locked --no-fail-fast -E 'test(agents::providers::codex::usage) | test(turn_settlement_tests) | test(interrupt) | test(working_directory) | test(missing_launch_directory) | test(send_recovery) | test(directory_error_alias) | test(master_directory_warning)'` — 130 selected, 130 passed, 4791 skipped. The original three reviewer probes are adopted in the committed tests. Full log, delta, fixture audit and hashes are archived under `evidence/`.
- 覆盖的审查维度：1–9 for eligible delta and direct consequences only: ownership and projection boundary, exact body/turn correctness, error and trust boundaries, log-only UI contract, naming/comments, locked-purpose scope, edge cases, complexity and regression coverage. No new qualifying issue remains.
- Verification boundary：Ready is this lane's code review verdict at the exact clean head above. The separately assigned final-head full gate and private real-Codex acceptance are pending on the author and are not certified by this review. No reviewer runtime, scratch HOME/data directory, build-slot lock or queue reservation was created.
- 考虑过但判定非 finding 的候选：No additional qualifying delta regression; the status/CLI consequences are one repaired root, not separate findings. Unchanged Round 1 coverage is carried forward.

## 代码就绪状态

- **判定**：Ready
- **收敛趋势**：R1-1 已核销；本轮新问题 0。
