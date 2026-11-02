# Review Round 2 — 代码审查

**分支**：fix/codex-rollout-final-answer @ ad9dc509
**基线**：origin/master @ 306b1773
**计划（如有）**：无
**锁定目标**：Recover the exact final answer of the matched Codex 0.164.0 rollout turn when missing launch-directory hooks cannot report it, and show that agent as Unavailable with the cause only in the client log; prove both with recorded-fixture TDD, a final-head full gate and private real-Codex acceptance.
**审查者**：clauderev5
**姿态**：standard
**日期**：2026-11-02

## 前轮问题核销

| # | 来源 | 问题 | 核销状态 | 证据 / 去向 |
|---|------|------|----------|-------------|
| 1 | R1-1 | Citation-only finals recovered as raw `<oai-mem-citation>` markup, diverging from the Stop hook | satisfactory | usage.rs:104-110 searches `event_msg` AgentMessage finals before `response_item`; usage.rs:150-157 maps a citation-only response item to `""`; new fixture hook-captured-0.164.0.jsonl and usage_test parity cases pin `""` for the recorded 36975 shape |
| 2 | R1-2 | Unavailable projection made interrupt and recovery treat a running turn as absent | satisfactory | status.rs:60-77 splits `activity_status()` (dialog/missing binary/native) from `shown_status()`; interrupt dispatch and `needs_stop` (control/agents.rs:321,404), `recovery_reason` (requests.rs:221) and the CLI waiter (cli/control/interrupt.rs:140-149, reading `activity_status` that `agent_json` now emits, inspect.rs:268) use activity; covered by interrupt_test, tests_test and send_recovery_test additions |

> 总结：R1-1、R1-2 satisfactory。

## 新问题与建议

无。

## 同步清单（CONSISTENCY drift，非阻塞）

无。

## 本轮探索区域

- Read 的源码文件：git diff 45fffccd..ad9dc509 (usage.rs, cli/control/interrupt.rs, control/agents.rs, control/inspect.rs, model/requests.rs, model/status.rs) and test/fixture additions; inspect.rs state builder feeding wait_for_stops
- 运行的测试：无
- 覆盖的审查维度：1, 2, 7 (delta only)
- 考虑过但判定非 finding 的候选：remaining shown_status readers (delivery retry identity, idle_agents finished, room_status, TUI rows) correctly stay on the presentation projection

## 代码就绪状态

- **判定**：Ready
- **收敛趋势**：2 → 0
