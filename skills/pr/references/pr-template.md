<!--
  Bus PR 模板。`pr` skill 读取本文件生成 PR 标题与正文，并用
  `skills/pr/scripts/pr_format_check.py` 机械校验最终标题与正文是否符合本模板。
  rebase 后立即创建的 Draft 使用 `--phase draft`，门禁与文档同步完成后的正文使用
  `--phase final`（默认）。两种阶段共享相同 section，区别只在 pending checkbox 契约。
  说明性内容（`>` 引用块与 HTML 注释）是给 AI 的指令，不得出现在最终 PR 里。
-->

## 🚨 重要提示

> # ⚠️ 每个 PR 只聚焦单一目的
>
> **若此 PR 含多个互不相关的变更，应拒绝生成 PR 标题/描述，并建议开发者拆成多个更小的 PR。**

# PR 标题

**格式**：`[类型] 简短描述`

**类型**：`feat | fix | refactor | perf | infra | chore | test | docs | build | arch`

> - `feat`：用户可感知的新功能（用户看不到变化的不算 feat）
> - `fix`：bug 修复
> - `refactor`：重构（行为不变）
> - `perf`：性能优化
> - `infra`：开发基础设施（脚本、CLI、devtool、workflow skill 等非用户可见代码）
> - `chore`：清理旧实验 / 旧 flag / 废弃功能
> - `test` / `docs` / `build`：测试 / 文档 / 构建·CI
> - `arch`：架构层面的改动

**示例**：

- `[feat] add room orchestration`
- `[fix] drain terminal palette replies on shutdown`
- `[infra] align workflow skills with the canonical architecture`

---

# PR 描述

## 摘要

- **主要变更**：[一句话概述核心改动]
- **大小**：`[XS | S | M | L | XL]`

> **大小说明**（衡量的是设计/审查复杂度，不是 diff 行数）：
>
> - `XS`：< 50 行、< 3 文件、0 新抽象
> - `S`：50–200 行、3–5 文件、0–1 新抽象
> - `M`：200–500 行、5–10 文件、1–3 新抽象
> - `L`：500–1000 行、10–20 文件、3–5 新抽象
> - `XL`：> 1000 行、> 20 文件、> 5 新抽象
>
> **机械性变更降级**：若 PR 绝大部分是纯机械操作（每处遵循同一条可机械验证的规则、不涉及业务/设计判断），大小至少降一档——审查者抽样几处确认规则一致即可，无需逐文件思考。典型：public 方法/字段改名导致全仓调用点跟改、签名变更机械跟改、批量修 lint 违规、codemod API 迁移、目录重组。触发机械变更的"源头"本身（新 lint 规则 / codemod / 新签名）不参与降级。
>
> Plan document (when applicable): for M or larger work, link `plans/xxx.md` at the end of the summary. A plan link is descriptive context, never a prerequisite.

## 目标

> `pr_goal_context.py` 生成本节与「非目标」节。有计划时，正文逐字采用每份计划的同名
> section body，并按参数顺序以一个空行连接；无计划时采用 agent 根据 PR 事实源撰写的目标。

[目标原文或 agent 撰写的目标]

## 非目标

> 与「目标」共用同一份机械生成的 context；没有需要声明的非目标时写「无」。

[非目标原文或无]

## 变更内容

> 汇总门禁通过时 `baseline...gated_head` 的代码 commit，按主题组织；不要罗列文件名。
> PR 打开后落盘的 `update-docs`-only commit 只由「文档同步」节表达，不重复进本节。

- [变更项 1]
- [变更项 2]

## 文档同步

> rebase 后立即创建 Draft 时，只写 `- [ ]` pending 项并用 `--phase draft` 校验。最终正文用
> `python3 skills/update-docs/scripts/docs_audit.py render-pr --audit <audit>` 的 stdout
> **完整替换本节（包括标题）**，再用 `--phase final` 校验；不要手工填写、翻译、重排或推断。

- [x] [renderer output]

## 自测 / Agent 测

> rebase 后立即创建 Draft 时，尚未执行的 readiness gate 可以保留为 `- [ ]`，并用
> `--phase draft` 校验。最终正文只逐条保留**合并前实际执行过**的自测与 Agent 测试，全部
> 勾选 `- [x]`；未执行的项移到「其他说明」，不得保留未勾选框，至少保留一项，并用
> `--phase final` 校验。每项写明命令或场景与结果，不得假装通过。

<!-- 示例：开始 -->
- [x] `just test` — passed
- [x] `just lint` — passed
- [x] Manual terminal check — passed against the recorded commit
<!-- 示例：结束 -->

## 截图 / GIF

> UI 变更为每个场景附截图或 GIF；非 UI 变更写「无 UI 变更」。

## 其他说明

> 未覆盖的验证、已知观感缺口、刻意不做的事写在此处；没有则写「无」。
