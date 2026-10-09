# Review Artifact Format

This document is the shared format source of truth for `review.md`, covering the plan review and PR code review modes. Final artifacts must be agent-anonymous; findings state only verifiable facts and carry no severity or disposition label.

## Shared output discipline

- The chat response equals `review.md` with the file-only bookkeeping removed; do not add greetings, an overall assessment, or a follow-up question before or after it.
- After writing `review.md`, both modes must run `python3 cli_extensions/review_artifact.py render-response`; the script fully validates the artifact, condenses the Round 2+ reconciliation table into its existing summary sentence, deletes `本轮探索区域`, and generates the only `chat-response.md`. The chat response equals that file verbatim and is no longer converted by hand by the agent.
- A section with no content contains only `无。`, without any account of how candidate issues were ruled out.
- An unresolved prior-round item keeps its original source number in the reconciliation table and is restated in full, with current evidence, under this round's `新问题与建议`; the same root cause must not be reopened under a new title.
- In Round 2+, when the prior round had no finding, the reconciliation table keeps only the fixed header and separator, and the summary must be exactly `> 总结：前轮无待核销 finding。`; when the table has data rows, that fixed empty summary must not be used. This rule is the same for the plan and PR modes.
- Reconciliation state semantics are fixed: `satisfactory` = the root cause is resolved; `rejected` = the author rejected it with evidence and the reasoning holds; `withdrawn` = the reviewer withdrew a false positive; `partially-addressed` = changed but the root cause remains; `not-addressed` = not handled and no valid reason; `disputed` = the author rejected it but the reasoning is insufficient, awaiting developer adjudication. The last three remain open.
- A finding is structured as location / observation / evidence / impact / optional remediation. A PR finding additionally names a code-review dimension.
- Disposition or severity labels such as `阻断`, `建议采纳`, `critical`, `high`, `medium`, `low` must not be written on a finding. Severity is used only for the mode's final verdict.

## Fixed actionable sections

Artifacts of both modes must contain exactly the following two headings, in template order, so author-side tooling can extract them deterministically:

```markdown
## 新问题与建议

## 同步清单（CONSISTENCY drift，非阻塞）
```

`新问题与建议` holds only SUBSTANTIVE findings that change the delivered result; with no finding, its body must be exactly `无。`.
`同步清单` normally holds only wording, naming, comment, or documentation drift that does not change the delivered result; PR code review additionally allows dimension 6
`XS` / `S` out-of-goal code slices as scope drift. With no drift, its body must be exactly `无。`. These two sections must not be renamed, merged,
reordered, or omitted.

## Plan-review round format

The artifact is written to `temp/review-plan/<branch_slug>/<plan_basename>/<reviewer>/round-NN/review.md`. The reviewer is read-only over the plan; archived decisions serve only as the consistency baseline and are not re-evaluated within a round.

```markdown
# Review Round <N> — 计划审查

**计划**：<plans/xxx.md>
**审查者**：<reviewer lane>
**姿态**：standard | devils-advocate
**日期**：<YYYY-MM-DD>

## 前轮问题核销

<!-- Round 1 writes `无。`. From Round 2 onward the file keeps the complete table; the chat response keeps only the summary sentence below.
     If the prior round had no finding, follow the shared empty-ledger rule in "Shared output discipline". -->

| # | 来源 | 问题 | 核销状态 | 证据 / 去向 |
|---|------|------|----------|-------------|
| 1 | R<round>-<index> | <summary> | satisfactory / rejected / withdrawn / partially-addressed / not-addressed / disputed | <diff hunk / path:line / author rationale; open items say `见新问题 N`> |

> 总结：<group source identifiers by reconciliation state; point open items to the corresponding new finding>。

## 新问题与建议

### <number. title><!-- append `(承 R<round>-<index>)` when carrying a prior finding -->
- **位置**：<plan section / exact location>
- **观察**：<fact>
- **证据**：<plan text / source / cross-reference, each with path:line>
- **影响**：<what happens to the executing agent if left unresolved>
- **可能的修复 / 选项**（可选）：<direct replacement text or options>

<!-- With no SUBSTANTIVE finding, write only `无。`. -->

## 同步清单（CONSISTENCY drift，非阻塞）

- <location → location: a point where wording for the same intent is out of sync, one per line> | 无。

## 本轮探索区域

<!-- File-only bookkeeping; not part of the chat response. Round 2+ records only prior-round reconciliation and direct consequences of this round's diff. -->

- Read 的源码文件：<paths>
- 交叉核对的测试场景：<scenarios>
- 走查的任务：<task ids / unique names>
- 考虑过但判定非 finding 的候选：<one terse line | 无>

## 计划就绪状态

- **判定**：可执行（Ready） | 需要完善（Needs Refinement） | 废弃（Abandon）
- **建议状态**：review-plan-complete | review-plan-in-progress | abandoned
- **收敛趋势**：<only the count and scope trend of `新问题与建议`; Round 1 uses `首轮`>
```

Fixed criteria for the plan verdict: it is 可执行 (Ready) when this round's `新问题与建议` is empty and every prior SUBSTANTIVE finding has been closed at the root cause; it is 需要完善 (Needs Refinement) when any unresolved or new SUBSTANTIVE finding exists; it is 废弃 (Abandon) only for a multi-purpose plan, a fundamentally wrong architecture, or one that cannot be salvaged locally. The consistency list does not block and does not trigger another round on its own.

## PR code-review round format

The artifact is written to `temp/review-pr/<branch>/<reviewer>/round-NN/review.md`. The reviewer is read-only over production code, with write access limited to test files (probe tests stay in the working tree and are not committed; see code-review-guide "Verification boundary"); with a related plan, record its goal verbatim; without a plan, record the branch-level locked goal verbatim.

For `review-pr --scope`, use `temp/review-pr/<branch>/scopes/<SCOPE_HASH>/<reviewer>/round-NN/review.md`
and the identical PR template below. Add the following two header fields before `**审查者**`:
`**范围哈希**：<64-character SCOPE_HASH>` and `**范围文件**：<canonical SCOPE_FILE>`.
Both are required together; branch artifacts omit both. They identify the whole-file chunk and its
probe allowlist, and are preserved verbatim by the deterministic renderer. Findings reference chunk
files, regardless of whether those lines appeared in the branch diff. The fixed sections, prior-round
ledger, proof evidence, verdicts, and response gate are unchanged.

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

<!-- Round 1 writes `无。`. From Round 2 onward the file keeps the complete table; the chat response keeps only the summary sentence below.
     If the prior round had no finding, follow the shared empty-ledger rule in "Shared output discipline". -->

| # | 来源 | 问题 | 核销状态 | 证据 / 去向 |
|---|------|------|----------|-------------|
| 1 | R<round>-<index> | <summary> | satisfactory / rejected / withdrawn / partially-addressed / not-addressed / disputed | <delta hunk / path:line / author rationale; open items say `见新问题 N`> |

> 总结：<group source identifiers by reconciliation state; point open items to the corresponding new finding>。

## 新问题与建议

### <number. title><!-- append `(承 R<round>-<index>)` when carrying a prior finding -->
- **位置**：<path:line>
- **维度**：<one code-review-guide dimension>
- **观察**：<fact>
- **证据**：<code / mechanism / cross-reference, each with path:line>; a finding in dimension 2 / 3 / 7 marks its proof status as the first item — `已证明：<test name @ test file relative path> — <failed assertion>` or `未证明`; other dimensions carry no mark
- **影响**：<what happens at execution or runtime if left unresolved; write it conditionally when marked `未证明`>
- **可能的修复**（可选）：<direct patch or design; a bug finding includes reproduction, fix, and regression test>

<!-- With no SUBSTANTIVE finding, write only `无。`. -->

## 同步清单（CONSISTENCY drift，非阻塞）

- <path:line: naming / comment / documentation drift; a dimension 6 XS/S slice also states its out-of-goal purpose, size, and evidence of how it differs from the locked goal; one per line> | 无。

## 本轮探索区域

<!-- File-only bookkeeping; not part of the chat response. Round 2+ records only prior-round reconciliation and direct consequences of this round's delta. -->

- Read 的源码文件：<paths>
- 运行的测试：<commands and results | 无>
- 覆盖的审查维度：<guide dimension numbers covered this round>
- 考虑过但判定非 finding 的候选：<one terse line | 无>

## 代码就绪状态

- **判定**：Ready | Needs Refinement | Abandon
- **收敛趋势**：<only the count and scope trend of `新问题与建议`; Round 1 uses `首轮`>
```

The PR verdict semantics are defined in one place, [code-review-guide "Three verdicts"](code-review-guide.md#three-verdicts); this template only fixes the legal enum and output structure and sets no separate Ready criterion.
