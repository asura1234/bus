# Review Artifact Format

This document is the shared format source of truth for plan reviews, PR code reviews, and task acceptance reviews. Final artifacts are agent-anonymous. Findings state verifiable facts and never carry severity or disposition labels.

## Shared output discipline

- The chat response is the `review.md` content after file-only bookkeeping is removed. Do not add greetings, an overall assessment, or a follow-up question.
- After writing `review.md`, every mode must run `python3 cli_extensions/review_artifact.py render-response`. The script validates the complete artifact, condenses the Round 2+ reconciliation table into its existing summary sentence, removes `本轮探索区域`, and generates the only valid `chat-response.md`. Return that file verbatim.
- An empty section contains only `无。`.
- An unresolved prior-round finding keeps its original source number in the reconciliation table and is restated completely, using current evidence, under `新问题与建议`. Never reopen the same root cause under a new title.
- In Round 2+, when the preceding round had no findings, keep only the fixed reconciliation table header and separator and use exactly `> 总结：前轮无待核销 finding。`. When the table has data rows, that empty summary is forbidden. This rule is shared by plan, PR, and task modes.
- Reconciliation states are fixed: `satisfactory` means the root cause is resolved; `rejected` means the author rejected it with sound evidence; `withdrawn` means the reviewer withdrew a false positive; `partially-addressed` means changes were made but the root cause remains; `not-addressed` means it was not handled and no valid reason was supplied; `disputed` means the author rejected it without sufficient evidence and developer adjudication is needed. The final three remain open.
- A finding is organized as location, observation, evidence, impact, and optional remediation. PR findings also name a code-review dimension. Task findings do not reuse the PR nine-dimension vocabulary.
- Findings must not contain disposition or severity labels such as `阻断`, `建议采纳`, `critical`, `high`, `medium`, or `low`. Severity influences only the mode's single final verdict.

## Fixed actionable sections

All three artifact modes contain these headings exactly once and in this order so author-side tooling can extract them deterministically:

```markdown
## 新问题与建议

## 同步清单（CONSISTENCY drift，非阻塞）
```

`新问题与建议` contains only SUBSTANTIVE findings that can change the delivered result. With no findings, its body is exactly `无。`. `同步清单` normally contains non-blocking wording, naming, comment, or documentation drift. PR review additionally permits dimension 6 out-of-goal `XS`/`S` code slices. With no drift, its body is exactly `无。`. These sections cannot be renamed, merged, reordered, or omitted.

## Plan-review round format

Write the artifact to `temp/review-plan/<branch_slug>/<plan_basename>/<reviewer>/round-NN/review.md`. The reviewer is read-only over the plan. Archived decisions are consistency inputs and are not re-decided in a review round.

```markdown
# Review Round <N> — 计划审查

**计划**：<plans/xxx.md>
**审查者**：<reviewer lane>
**姿态**：standard | devils-advocate
**日期**：<YYYY-MM-DD>

## 前轮问题核销

<!-- Round 1 writes `无。`. From Round 2 onward, the file retains the complete table;
     the chat response keeps only the summary below. If the prior round had no
     finding, follow the shared empty-ledger rule. -->

| # | 来源 | 问题 | 核销状态 | 证据 / 去向 |
|---|------|------|----------|-------------|
| 1 | R<round>-<index> | <summary> | satisfactory / rejected / withdrawn / partially-addressed / not-addressed / disputed | <diff hunk / path:line / author rationale; open items say `见新问题 N`> |

> 总结：<group source identifiers by reconciliation state; point open items to the corresponding new finding>。

## 新问题与建议

### <number. title><!-- append `(承 R<round>-<index>)` when carrying a prior finding -->
- **位置**：<plan section / exact location>
- **观察**：<fact>
- **证据**：<plan text / source / cross-reference, each with path:line>
- **影响**：<what happens to the implementing agent if left unresolved>
- **可能的修复 / 选项**（可选）：<direct replacement text or options>

<!-- With no SUBSTANTIVE finding, write only `无。`. -->

## 同步清单（CONSISTENCY drift，非阻塞）

- <location → location: wording that no longer expresses the same intent> | 无。

## 本轮探索区域

<!-- File-only bookkeeping; omitted from chat. Round 2+ records only prior-finding
     reconciliation and direct consequences of the current delta. -->

- Read 的源码文件：<paths>
- 交叉核对的测试场景：<scenarios>
- 走查的任务：<task ids / unique names>
- 考虑过但判定非 finding 的候选：<one terse line | 无>

## 计划就绪状态

- **判定**：可执行（Ready） | 需要完善（Needs Refinement） | 废弃（Abandon）
- **建议状态**：review-plan-complete | review-plan-in-progress | abandoned
- **收敛趋势**：<only the count and scope trend of `新问题与建议`; Round 1 uses `首轮`>
```

The plan verdict is fixed: `Ready` requires an empty current `新问题与建议` section and root-cause closure of every prior SUBSTANTIVE finding. Any new or unresolved SUBSTANTIVE finding yields `Needs Refinement`. Only a multi-purpose plan, fundamentally wrong architecture, or a plan that cannot be repaired locally yields `Abandon`. The consistency list neither blocks nor triggers another round.

## PR code-review round format

Write the artifact to `temp/review-pr/<branch>/<reviewer>/round-NN/review.md`. The reviewer is read-only over production code; only probe tests may be written and they remain uncommitted. With a related plan, record its goal verbatim. Without one, record the branch-level locked goal verbatim.

```markdown
# Review Round <N> — 代码审查

**分支**：<branch> @ <short HEAD>
**基线**：<base ref> @ <short SHA>
**计划（如有）**：<plans/xxx.md | 无>
**锁定目标**：<plan goal or branch-level `.locked-goal` verbatim; continuation lines run until the next `**key**：` or first H2>
**审查者**：<reviewer lane>
**姿态**：standard | devils-advocate
**日期**：<YYYY-MM-DD>

## 前轮问题核销

<!-- Round 1 writes `无。`. From Round 2 onward, retain the complete table in the
     file; chat keeps only the summary. Follow the shared empty-ledger rule. -->

| # | 来源 | 问题 | 核销状态 | 证据 / 去向 |
|---|------|------|----------|-------------|
| 1 | R<round>-<index> | <summary> | satisfactory / rejected / withdrawn / partially-addressed / not-addressed / disputed | <delta hunk / path:line / author rationale; open items say `见新问题 N`> |

> 总结：<group source identifiers by reconciliation state; point open items to the corresponding new finding>。

## 新问题与建议

### <number. title><!-- append `(承 R<round>-<index>)` when carrying a prior finding -->
- **位置**：<path:line>
- **维度**：<one code-review-guide dimension>
- **观察**：<fact>
- **证据**：<code / mechanism / cross-reference, each with path:line>; findings in dimensions 2, 3, or 7 begin with either `已证明：<test @ relative test path> — <failed assertion>` or `未证明`; other dimensions have no proof-status label
- **影响**：<runtime consequence; use conditional language for `未证明`>
- **可能的修复**（可选）：<direct patch or design; a bug finding includes reproduction, fix, and regression test>

<!-- With no SUBSTANTIVE finding, write only `无。`. -->

## 同步清单（CONSISTENCY drift，非阻塞）

- <path:line: naming/comment/documentation drift; a dimension 6 XS/S slice also states its out-of-goal purpose, size, and locked-goal evidence> | 无。

## 本轮探索区域

<!-- File-only bookkeeping; omitted from chat. Round 2+ records only prior-finding
     reconciliation and direct consequences of the current delta. -->

- Read 的源码文件：<paths>
- 运行的测试：<commands and results | 无>
- 覆盖的审查维度：<guide dimension numbers covered this round>
- 考虑过但判定非 finding 的候选：<one terse line | 无>

## 代码就绪状态

- **判定**：Ready | Needs Refinement | Abandon
- **收敛趋势**：<only the count and scope trend of `新问题与建议`; Round 1 uses `首轮`>
```

The PR verdict meaning is defined only by `code-review-guide.md`; this template fixes the legal enum and output structure without creating another readiness rule.

## Task-acceptance round format

Write the artifact to the `--output` path under `temp/review-task/<full-plan-slug>/task-<id>-<name>/round-NN/review.md`. The full plan slug derives from the complete repository-relative plan path without its extension. Task review verifies only that task's completion, plan adherence, file isolation, direct contracts, and gate evidence; it is not final PR review.

```markdown
# Review Round <N> — 任务验收

**计划**：<plans/xxx.md>
**任务**：任务<id>：<unique plan task name>
**实际触及文件**：<N> files；canonical list: <SCOPE_INPUTS>#files
**Task report**：<generation-NN/report.md>
**GATE_EVIDENCE_HASH**：<review_round.py output hash>
**SCOPE_HASH**：<review_round.py output hash>
**输出 lane**：<temp/review-task/.../round-NN/review.md>
**日期**：<YYYY-MM-DD>

## 前轮问题核销

<!-- Round 1 writes `无。`. From Round 2 onward reconcile only findings from this
     task lane. Follow the shared empty-ledger rule. -->

| # | 来源 | 问题 | 核销状态 | 证据 / 去向 |
|---|------|------|----------|-------------|
| 1 | R<round>-<index> | <summary> | satisfactory / rejected / withdrawn / partially-addressed / not-addressed / disputed | <task delta / path:line / author rationale; open items say `见新问题 N`> |

> 总结：<group source identifiers by reconciliation state; point open items to the corresponding new finding>。

## 任务契约对照

- **目标与约束**：<status and evidence>
- **拥有文件与实际触及文件**：<all inside owner | out-of-scope paths and evidence>
- **produces / consumes**：<direct upstream/downstream contract status | 无>
- **验收闸门**：<verification of exact command/workspace/exit/test count/coverage/log hash>
- **计划偏差**：<plan statement / actual result / reason | 无>

## 新问题与建议

### <number. title><!-- append `(承 R<round>-<index>)` when carrying a prior finding -->
- **位置**：<task field / actual path:line / gate evidence>
- **观察**：<fact>
- **证据**：<task contract / code / test output / direct dependency, with path or command>
- **影响**：<why this task cannot be accepted independently>
- **可能的修复 / 选项**（可选）：<owner-local correction; explain when the task graph must reopen>

<!-- With no SUBSTANTIVE finding, write only `无。`. -->

## 同步清单（CONSISTENCY drift，非阻塞）

- <task-contract or owner-document wording drift> | 无。

## 验收证据

- 实际触及文件：<N> files；已按 SCOPE_HASH 核对 <SCOPE_INPUTS>#files
- Task report / gate logs：<report path, evidence hash, commands, and results>
- Reviewer 窄复现：<not run | exact command and reason/result>
- 读取的直接上游产物：<path or contract | 无>

## 任务就绪状态

- **判定**：Ready | Needs Refinement | Plan Repair Required
- **修复原因**：无 | upstream-contract | owner-graph-contract | developer-decision
- **上游任务**：无 | 任务 N
- **收敛趋势**：<only the count and scope trend of `新问题与建议`; Round 1 uses `首轮`>
```

Task verdicts are fixed: `Ready` requires the goal, constraints, owner, direct contracts, and gate evidence to hold, with no current finding and all prior SUBSTANTIVE findings closed at root cause. Owner-local repairs yield `Needs Refinement`. A broken upstream contract, incomplete owner/dependency/task contract, or required developer decision yields `Plan Repair Required` with the structured reason. Only `upstream-contract` may name an upstream task. Consistency drift does not block or trigger another round.
