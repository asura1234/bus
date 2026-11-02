# Review Round 1 — 代码审查

**分支**：fix/codex-rollout-final-answer @ 45fffccd
**基线**：origin/master @ 306b1773
**计划（如有）**：无
**锁定目标**：Recover the exact final answer of the matched Codex 0.164.0 rollout turn when missing launch-directory hooks cannot report it, and show that agent as Unavailable with the cause only in the client log; prove both with recorded-fixture TDD, a final-head full gate and private real-Codex acceptance.
**审查者**：clauderev5
**姿态**：standard
**日期**：2026-11-02

## 前轮问题核销

无。

## 新问题与建议

### 1. Citation-only 0.164.0 finals are recovered as raw `<oai-mem-citation>` markup, unlike the Stop-hook reply for the same shape
- **位置**：src/agents/providers/codex/usage.rs:114
- **维度**：1. Architecture and design
- **观察**：`completed_final_text` searches backwards from `task_complete`, so for a turn that has both a completed `AgentMessage` (`Text` = `""`, citations held structurally in `memory_citation`) and the later assistant `response_item` (`output_text` = the citation block), the response item wins. The doc comment states this preference on purpose, and `rollout_0164_recovers_the_actual_four_lost_finals` pins the citation XML as the expected reply for three of the four turns. The Stop-hook route returns `""` for the same turn shape. So one turn yields different reply text depending on which capture route settles it.
- **证据**：src/agents/providers/codex/usage.rs:111-114（doc comment: "Prefer that newer response when both representations exist"）; src/agents/providers/codex/tests/usage_test.rs:318-334（expects the `<oai-mem-citation>…</oai-mem-citation>` text for turns 01a19a4c/01a19a5b/01a19a71）. Recorded control from the same launch: temp/reply-not-captured/codexrev5-rollout.jsonl:205-209 (turn 01a19a28-e395) has the same shape: `AgentMessage` `Text` `""` with `phase:"final_answer"`, then a completed `response_item` containing only an `<oai-mem-citation>` block, then `task_complete.last_agent_message:null`. Its working Stop hook settled request 36975 with `reply.text:""` (temp/reply-not-captured/history-before-37106.json, request 36975 `"provider_turn_id":"01a19a28-e395-…","text":""`), via src/agents/providers/codex/hooks.rs:97-101 (`optional_text(last_assistant_message)`).
- **影响**：When hooks are lost, Bus forwards the turn-final text to the requester. The requester then receives internal Codex memory-citation markup (`MEMORY.md:276-277|note=[…]`) as the "reply", where the hook route would have given `""`. That is not the turn's user-visible final answer. It also exposes the agent's memory-file references to another agent's composer. The fix target "exact final answer of the matched turn" then depends on which route settled the turn.
- **可能的修复**：Treat the completed `AgentMessage` item (`Text` parts) as the authoritative final for a turn, matching what Codex renders and what the Stop hook reports. Use the `response_item` only when no `AgentMessage` final exists for that turn, and strip a pure `<oai-mem-citation>` block from it. Change the three fixture expectations to `""`, and add a case asserting that hook and rollout routes agree for fixture line 205-209's shape.

### 2. The Unavailable projection makes `bus agent interrupt` refuse a turn that is still running
- **位置**：src/messaging/model/status.rs:61
- **维度**：2. Correctness
- **证据**：未证明; `shown_status()` now returns `Unavailable` whenever `working_directory_error` is set, and the PR keeps the native `status` (Working) so the in-flight turn can still settle (doc comment status.rs:56-59). `interrupt_agent` dispatches on `shown_status()` (src/messaging/coordinator/control/agents.rs:321) and `Unavailable` falls into the `other =>` arm (agents.rs:382-389): outcome `failed`, "agent X is unavailable; there is no turn to interrupt". `needs_stop` (agents.rs:404) likewise only recognises Working/Blocked/Idle. The recorded incident shows such agents keep running whole turns after the cwd is removed (temp/reply-not-captured/finding.md, turns at rollout lines 537-606 … 765-780). Other control/recovery readers of `shown_status()` also change meaning for a natively Working agent, e.g. `recovery_reason` no longer early-returns for Working (src/messaging/model/requests.rs:220-225).
- **观察**：The status that is shown and the status that decides control actions are now the same field. "Show Unavailable" therefore also changes what operations see: a Working agent without a cwd is treated as having no turn.
- **影响**：If an agent's launch cwd is removed while it runs a turn, the orchestrator cannot stop that turn with `bus agent interrupt`. The command fails and says there is nothing to interrupt, while the provider keeps working. That is the case where a stop is most needed. Queued requests behind it may also start to report stall reasons while it is still natively Working.
- **可能的修复**：Keep `shown_status()` as the presentation projection (rows, room status, `inspect`) and have control paths (`interrupt_agent`/`needs_stop`, `recovery_reason`, reply-retry identity) read the native `status` plus `dialog`/`missing_binary` as before; or add an `Unavailable`-with-native-Working arm to the interrupt match. Regression test: an agent with `status=Working` and `working_directory_error=Some(..)` → `interrupt` outcome `interrupting`.

## 同步清单（CONSISTENCY drift，非阻塞）

无。

## 本轮探索区域

- Read 的源码文件：src/agents/providers/codex/usage.rs; src/agents/providers/codex/hooks.rs; src/agents/providers/codex/tests/usage_test.rs; src/agents/providers/codex/tests/fixtures/*; src/messaging/model/status.rs; src/messaging/model/requests.rs; src/messaging/model/idle_agents.rs; src/messaging/coordinator/control/agents.rs; src/messaging/coordinator/delivery.rs; src/messaging/coordinator/tests/turn_settlement_test.rs; src/messaging/coordinator/tests/working_directory_test.rs; src/client/rooms/tests/notices_test.rs; temp/reply-not-captured/finding.md, codexrev5-rollout.jsonl:150-211, history-before-37106.json
- 运行的测试：无（author's final-head full gate held both build slots and required an unchanged tree; no probes written）
- 覆盖的审查维度：1, 2, 3, 4, 5, 6, 7, 8, 9
- 考虑过但判定非 finding 的候选：wrong-turn capture (both item kinds filtered by turn_id and bounded to events before the matched task_complete — ok); empty final `""` vs absent text (handled); error precedence (preserved); Unavailable recovers when cwd returns (working_directory_test asserts restored status); TUI shows no error text (notices_test asserts); single purpose (both fixes are in the locked goal)

## 代码就绪状态

- **判定**：Needs Refinement
- **收敛趋势**：首轮
