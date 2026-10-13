<!--
  Bus PR 标题规则与正文写作指引。正文的 section、占位符和自测 checkbox 只定义在
  `.github/pull_request_template.md`（GitHub 新建 PR 时也用它），本文件不重复 section 结构。
  `skills/pr/scripts/pr_format_check.py` 从本文件读标题类型，从 GitHub 模板读正文结构。
  `pr` 不运行门禁：自测 checkbox 有任一项 pending 时用 `--phase draft`，全部勾选时用
  `--phase final`（默认）。
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

以 `.github/pull_request_template.md` 为骨架，按顺序填满它的每个 `##` section，删掉 HTML 注释，
替换掉每个 `[…]` 占位符。下面按 section 补充 AI 写作规则。

### 摘要

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

### 目标

> `pr_goal_context.py` 生成本节与「非目标」节。有计划时，正文逐字采用每份计划的同名
> section body，并按参数顺序以一个空行连接；无计划时采用 agent 根据 PR 事实源撰写的目标。

### 非目标

> 与「目标」共用同一份机械生成的 context；没有需要声明的非目标时写「无」。

### 变更内容

> 汇总 `baseline...head` 的 commit，按主题组织；不要罗列文件名。

### 自测 / Agent 测

> GitHub 模板里的每个 checkbox（`e2e-test`、`gate-and-fix`）都必须保留，且以该名字开头：
>
> - 调用方提供了该项证据（证明当前 head 的 SHA 加报告路径）时勾选，写成
>   ``- [x] gate-and-fix — PASS, head `<sha>`, report `<path>` ``；
> - 否则保持 pending，写成 `- [ ] gate-and-fix — pending`。
>
> 调用方提供的其他已执行验证（如 `just test`、`just lint`）作为额外的 `- [x]` 项追加在后面，
> 每项写明命令或场景与结果，不得假装通过。

### 截图 / GIF

> UI 变更为每个场景附截图或 GIF；非 UI 变更写「无 UI 变更」。

### 其他说明

> 未覆盖的验证、已知观感缺口、刻意不做的事写在此处；没有则写「无」。
