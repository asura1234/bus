# Review Round 1 — 代码审查

**分支**：fix/codex-rollout-final-answer @ 45fffccda08b626b111afbe61aa77b192c0c9f1d
**基线**：origin/master @ 306b177332aaefd3addfbbb6c6f1cafb1b3b8a7c
**计划（如有）**：无
**锁定目标**：Recover the exact final answer of the matched Codex 0.164.0 rollout turn when missing launch-directory hooks cannot report it, and show that agent as Unavailable with the cause only in the client log; prove both with recorded-fixture TDD, a final-head full gate and private real-Codex acceptance.
**审查者**：codexrevf
**姿态**：standard
**日期**：2026-11-02

## 前轮问题核销

无。

## 新问题与建议

### 1. Missing-cwd availability suppresses control of a still-running turn and expires its queue

- **位置**：src/messaging/model/status.rs:61
- **维度**：2 — Correctness
- **观察**：The new `working_directory_error` override makes `shown_status()` Unavailable while native activity and a bound turn remain intact. Operational consumers also use that presentation value: explicit interrupt dispatch at `src/messaging/coordinator/control/agents.rs:321`, room interrupt selection at `src/messaging/coordinator/control/agents.rs:404`, and the busy-queue expiry guard at `src/messaging/model/requests.rs:220`. The control state exposes that same value at `src/messaging/coordinator/control/inspect.rs:267`; the CLI waiter treats it as a failed stop at `src/cli/control/interrupt.rs:145`.
- **证据**：已证明：`missing_launch_directory_preserves_explicit_interrupt` @ `src/messaging/coordinator/tests/interrupt_test.rs:458` — actual `failed` / “there is no turn to interrupt”, expected `interrupting`; `missing_launch_directory_preserves_room_interrupt` @ `src/messaging/coordinator/tests/interrupt_test.rs:492` — actual `agents: []`, expected the busy agent with one Esc dispatch; `missing_launch_directory_preserves_busy_queue` @ `src/messaging/model/tests/send_recovery_test.rs:443` — a queued request expires despite native Working. All three fail at the exact reviewed head in `evidence/review-probes-red.log`, with command and attribution in `evidence/manifest.json`. The saved append-only patches are in `../probes/`. The companion producer/CLI regression log `evidence/activity-control-red-2.log` also records an absent activity field and `interrupt_failed` / “agent a4 became unavailable”.
- **影响**：Removing an agent's launch directory during a running turn prevents the developer from stopping it through explicit or room interruption, although the provider still runs. Requests queued behind that busy agent can be abandoned by the ordinary stall timer. Fixing coordinator dispatch alone leaves the CLI waiter falsely reporting failure. These consequences violate the intended availability indication while preserving native activity for in-flight work.
- **可能的修复**：Keep a shared activity projection that retains the existing native/dialog/binary semantics, and derive shown availability from it. Use activity for interrupt targeting, dispatch, and busy-queue recovery; expose it to the direct CLI waiter with compatibility for older control responses. Keep the TUI Unavailable and the cwd cause in the client log. Retain the three supplied regressions plus the producer/CLI checks; cover bound Idle turns, single stop-key dispatch, actual process unavailability, busy queue retention, and recovery after cwd restoration.

## 同步清单（CONSISTENCY drift，非阻塞）

无。

## 本轮探索区域

- Read 的源码文件：Both commits (`70c0e8b8`, `45fffccd`) and all eight changed files; direct Codex turn attribution/capture callers, cwd sampling/recovery, client notices/sidebar, coordinator interrupt/inspection, model request expiry/recovery, and CLI interrupt waiting. The reviewed source is pinned in `evidence/pinned-source/`; ongoing uncommitted author repairs are not this verdict's subject.
- 运行的测试：No reviewer build, cargo run, full gate, live acceptance, or source write. The author applied the two saved append-only test patches and confirmed exact head/command in Bus message 37473: `cargo nextest run --locked --no-fail-fast -E 'test(missing_launch_directory_preserves_) | test(rollout_0164_matches_the_recorded_stop_reply)'` under slot 1. Four selected tests failed, including all three projection cases supplied by this lane; the fourth is the separately tracked Stop/citation parity case. The author explicitly retains/adopts all three projection cases, so reviewer cleanup/removal is neither required nor performed. No reviewer runtime, slot lock, or queue reservation remains.
- 覆盖的审查维度：1–9. Architecture/ownership and direct consumer boundaries; correctness/edge cases; trust and exact-turn attribution; log-only error handling; naming/comments; the locked goal's single repair purpose; branch complexity; regression/evidence coverage. Fresh-eyes read-only review independently identified the same control/queue root and has completed; no additional root was admitted.
- Fixture audit：`fixture-provenance.json` verifies all 16 fixture lines byte-for-byte against original rollout lines 537, 602, 603, 606, 625, 681, 682, 685, 702, 744, 745, 748, 765, 776, 777, 780. Original SHA-256: `b325dc3991004a40a65e552a15d9be8ea2c01cb17a1abd18bc0226e6a26feef9`. Exact-turn filtering, completion boundaries, legacy completion text, foreign/running/commentary records, explicit empty text, missing text, and provider-error precedence were inspected. Citation-only output versus captured native Stop is already the unified author's canonical APPLY item 1; it is not duplicated as another finding here.
- Recovery/UI contract：Inspected the existing directory-reappearance path, raw status preservation, warning deduplication, client-log cause, and sidebar/control projection. The status projection regression above is the admitted consumer break; no additional TUI error text was introduced.
- Evidence boundary：Historical focused TDD logs are author evidence, not reviewer execution or a final-head full gate. `rebase-touched-paths.diff` records identical touched paths across the author's rebase. The first full-gate artifact is FAIL from its documented environment setup; successful final-head full gate and private real-Codex acceptance are not established by this review. Repair green logs do not retroactively change this pinned-head R1 verdict.
- Unified triage：`evidence/author-triage.md` accepts the same status-projection root as canonical APPLY item 2, including CLI consumption; the three consequences require one repair. No other review lane's report was read for discovery, and no duplicate repair is requested. The separate native Stop/citation body root is already tracked there.
- 考虑过但判定非 finding 的候选：No additional qualifying architecture/security/log/naming/scope/complexity issue beyond the canonical status projection root; fixture provenance is verified. Full gate/live acceptance remain author deliverables, not speculative source findings.

## 代码就绪状态

- **判定**：Needs Refinement
- **收敛趋势**：首轮；1 canonical status-projection finding, supported by three red behavioral probes and its direct CLI consequence.
