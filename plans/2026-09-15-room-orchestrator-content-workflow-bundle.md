# 交付智能体主导的编排器内容包与动态 SOP

**状态**：plan-execution-in-progress
**作者**：dylanliu8949
**创建日期**：2026-09-15
**基于提交**：7bc910f2aba2f79ca4ddbc83e76a3c44f8fa88d5
**分支**：master
**前置任务（如适用）**：完成、重新审查并验证 `plans/2026-09-15-room-orchestrator-agent-led-harness.md` 的 closed Orchestrator query/operation surface、Room Brief bootstrap/confirmation authority、`ROOM_AGENT_CONTENT_INTERFACE_V1` 与 `TRUSTED_ROOM_ASSIGNMENT_V1`；该前置计划已删除 synthetic scripted-provider acceptance driver，本计划不恢复或替代它
**后续任务（如适用）**：无

> **语言无关说明**：本模板适用于本仓库涉及的任意目标语言。Rust 是产品主语言，workflow helper 使用 Python，少量集成资源使用 TypeScript。下方代码片段示例必须改用任务的目标语言表达。

> **状态说明**：
> 状态值为 `<phase>-<phase-state>` 的组合：6 个 phase 按下表顺序线性推进；前 5 个 phase 各有 `in-progress` 和 `complete` 两个 phase-state，第 6 个 phase `merge` 只有 `merge-complete`（合并是瞬时操作，没有“进行中”的中间态——`code-review-complete` 之后下一个状态就是 `merge-complete`）。完整枚举共 12 值：`<phase>-in-progress` / `<phase>-complete` × 5 + `merge-complete` + 特殊终态 `abandoned`（任意阶段可手动写入，表示计划废弃）。该字段是机器可读的工作流门控，请勿引入此列表以外的值。
>
> | # | phase | 含义 | `in-progress` 写入时机 | `complete` 写入时机 |
> |---|-------|------|-----------------------|---------------------|
> | 1 | `create-plan` | 计划文档撰写 | `/create-plan` 启动 | 落盘等待审查 |
> | 2 | `review-plan` | 计划审查 | `/address-review-comments` 首次处理评审时（`/review-plan` 对计划只读、不写状态） | 审查通过（`/execute-plan` 的最低门槛） |
> | 3 | `plan-execution` | 任务图实施 | `/execute-plan` 启动 | 全部任务验收 + 自动 EXIT CHECK 达标，并以最后一步 `commit-and-push` 提交、推送精确 tree |
> | 4 | `manual-test` | 人工手动测试 | 开发者手动 | 开发者手动 |
> | 5 | `code-review` | 最终代码审查 | 人工测试完成后开始 | 审查通过 |
> | 6 | `merge` | PR 合入 `master` | — （无 in-progress） | 合并完成后由开发者或合并流程写入（终态） |
>
> **门控规则**：
> - `/review-plan` 与 `/execute-plan` 只依赖本文档格式和状态，不依赖 `/create-plan` session 或额外私有状态；开发者手写但通过同一格式/gate 的计划同样合法
> - `/execute-plan` 在自动 gate 全绿后以 `commit-and-push` 作为最后一步，结束于已提交并推送到当前具名分支的精确 tree；之后依次由开发者人工验证、`/pr` 创建或更新面向 `master` 的 PR、`/review-pr` 审查。只有开发者明确要求时才直接落盘 `master`
> - 本仓库没有校验计划状态的 CI workflow，`状态` 由上述 workflow skill 自己消费。已进入 `plan-execution-complete` 及之后状态的计划文档视为已用，不得被新 PR 复用；需要新工作时新建计划文档

> **⚠️ 不可修改**：以下规则部分必须包含在每个计划文档中，AI 和开发者不得修改此部分。

<!-- 规则优先级：开始 - 此部分不可修改 -->
## 规则优先级

1. 开发者在对话中的最新明确要求。
2. 触及范围内实际存在的 `AGENTS.md`。
3. 本计划的目标、非目标与已归档决策。
4. 本模板、计划指南与代码现状。

事实必须区分为：当前源码已验证、开发者明确决定、待验证假设、延期工作。POC 的 ceiling 假设不得写成生产承诺。

- 计划生成规则和计划执行规则优先于模型的隐式行为
- 当任务指令与计划生成规则或计划执行规则冲突时，必须遵循这些规则
- 如果由于任务约束无法遵循计划生成规则或计划执行规则中的某条规则，应暂停并请求澄清，而不是猜测
- 如需覆盖这些规则，应更新计划模板文档，而不是在单个计划文档中覆盖
<!-- 规则优先级：结束 -->

## 目标

交付驱动 agent-led room orchestrator 的版本化内容包，包括 harness-level identity/system policy、domain-neutral `agent.md`、Mermaid-first `workflow-template.md`、`create-workflow` / `execute-workflow` skills、标准 SWE SOPs 与 coding-agent Room Brief 契约，使智能体能够自主解释、修订和执行动态流程而不承担技术实现工作。

> **重要**：
> - **目标与非目标共同标示本计划的意图（intent）与范围（scope）**：目标声明**要做什么 / 交付什么**（**强制**——写任何计划正文前必须先清晰设定，见 create-plan 的 GOAL GATE），非目标声明**刻意不做什么**（**可选**——未声明即视为无、AI 不问不猜，但**一旦声明即被强制执行**）；二者一起把计划的意图与边界钉死，是后续 create / review / execute 全程锁定的地基。
> - 计划必须**单一目标**。判断标准是**目标是否内聚**，不是任务数量——一个 XXXL 计划可以有多个粗粒度任务，只要它们共同服务同一个目标，它就仍是单一目标。
> - 类比：「建一座动物园」是单一目标，即使内部有建狮笼、建鸡舍、修围栏等多个任务，它们仍共同拼成一个成果。若顶层目标是多件互不相关的事，则应拆成多份单一目的计划。
> - 目标陈述聚焦**做什么 / 交付什么**（结果），不描述**怎么做**（实现细节留给后续章节）。
> - 计划一律 one-shot 执行、执行后再做 e2e 验证、一个计划一个 PR，与大小无关；不要把「单一目标 + 大」误当成「多目的」而拆散。
> - **目标在 create-plan 阶段定稿后即锁定**：只有开发者可修改。plan review / 任何 agent **不得推翻、扩张、缩小或重新定义**它，只能检查计划正文是否服务于该目标（详见 [`docs/guides/plan-review-guide.md`](../guides/plan-review-guide.md)「工作流结构是既定常量」）。

## 非目标

- 不修改前置 harness 的 provider、journal、tool registry、permissions、persistence、TUI、control protocol 或 safety policy；content 不能新增 tool 或让 harness 理解 workflow。
- 不让 orchestrator 检查、编写或修复技术实现，不让其从源码自行判断技术正确性、挑选 patch 或替代 coding agent；技术问题必须转发。
- 不增加 shell、source/diff/test/Git、任意文件读取、raw terminal-input、self-approval 或 cross-room capability；prompt 只是 defense-in-depth，hard authority 仍由 harness tool surface 限制。
- 不实现 executable workflow runner、固定 transition table、graph parser、领域 state machine、Appium test generator、Mermaid renderer 或 visual editor。
- 不实现 future best-of-N skill 本体；现有 SOP 可让 orchestrator 收集 N 份独立建议并明确委派 author agent 从完整候选中选择。
- 未经开发者明确授权，不执行 live paid DeepSeek call；正常 gate 只使用 scripted fake provider 和 fixed fixtures。
- 不允许 model/chat 直接把 `.bus/temp` 覆盖到 `.bus/standard`；promotion 仍要求 exact human action。
- 不把 orchestrator-only skills 暴露给 coding agents；只修改 canonical `skills/` sources 与其 derived symlink views。

> **重要**：
> - **可选，但设了就强制**：开发者未显式声明即写「无」——AI **不问、不猜、不外推**（宁可留「无」，不要编）；**一旦声明了任一条非目标，它即被强制执行**（往其方向推进的评审意见一律驳回，见下）。这是与目标的关键差别：目标**强制必设**，非目标**可留空、但设了不可犯**。
> - 与目标一样**锁定**：定稿后只有开发者可改；plan review / 任何 agent 不得新增、扩张或重新定义非目标。
> - 往非目标方向推进的评审意见（如「顺便也做 X」而 X 正是某条非目标）= 扩范围熵，一律驳回（见 [`docs/guides/plan-review-guide.md`](../guides/plan-review-guide.md) 与 [`docs/guides/code-review-guide.md`](../guides/code-review-guide.md)）。

## 当前状态分析

- **当前源码已验证（current HEAD，前置计划已完成）**：`src/bus/orchestrator/` 已存在。`build.rs` 只校验并打包 `src/bus/orchestrator/content/test-agent-led/manifest.json`；每个 manifest entry path 必须是 selector 目录内单一 `.md` 文件名（不能嵌套），单文件上限 64 KiB、总量上限 256 KiB。`src/bus/orchestrator/content.rs` 对 `ContentSelector::Production` 固定返回 `ProductionUnavailable`，因此 production bundle 需要本计划的最小 selector activation，不能只落盘静态文件。
- **当前源码已验证**：closed `RoomQuery` 恰为 `InspectWork`、`WaitForChange`、`ReadAgent`（`VisibleViewport` 或 positive-N `RecentTail`）、`ObservePermissionPrompt`、`ReadWorkflowDraft` 与 `ReadContent`；closed `RoomOperation` 恰为 `ProposeRoomBrief`、`SendMessage`、`AbandonIdleRequest`、`PersistWorkflowDraft`、`PromoteWorkflowDraft`、`AcquireResource`、`ReleaseResource` 与 `ApprovePermissionOnce`。前置计划已删除 artifact/codebase-outline queries 与 agent lifecycle、steering、queue disposition、coordination、Human-request operations；content 不得引用、别名或模拟这些已删除 token。Work evidence 只经 `InspectWork` 返回的 `WorkSettlement` facts 暴露，技术 artifact 由 coding agent 在 reply 中报告。
- **当前源码已验证**：没有任何 query 读取 `.bus/standard`；runtime 只在 developer approval 与 `PromoteWorkflowDraft` settlement 时读取或写入该目标。`ReadContent` 只读 embedded bundle 的 `system/system`、`agent/agent`、`index/index`、`skill/<name>` 与 `reference/<name>`；`ReadWorkflowDraft` 只读本 room 已由 `PersistWorkflowDraft` 登记的 `.bus/temp` draft。
- **当前源码已验证**：`.gitignore` 忽略 `/.bus/`；`.bus/temp` 与 `.bus/standard` 是本地 runtime workspace，任何 checked-in `.bus/` deliverable 都不会进入 PR。
- **当前源码已验证**：每次 model request 只注入 content 的 system、agent 与 index；skill/reference body 必须由 model 主动 `ReadContent`。Human 确认 Room Brief 前，Orchestrator 唯一可结算的是 unlocked room 上的 `ProposeRoomBrief`，所有 query（含 `ReadContent`）与其他 operation 都因缺少 grant 被拒绝；首次 exact Human confirmation 才激活固定 baseline capabilities。Brief 一旦 locked，`ProposeRoomBrief` 被拒绝，也不存在 update 或 unlock operation。
- **当前源码已验证**：`bus assignment verify --frame FRAME`（经 `BUS_BINARY` 调用时为 `--bus assignment verify --frame FRAME`，见 `src/main.rs`）要求 frame；completed verification 输出 `verified` 或 `invalid` JSON 并 exit 0，缺失 discovery 报 `invalid`，CLI 不可达 `absent`。
- **当前源码已验证**：canonical coding-agent skills 位于 `skills/`，`.agents/skills` 与 `.claude/skills` 是 derived symlink views；修改必须落到 canonical source。
- **当前源码已验证**：现有 create/review/PR skills 分别拥有自己的 Goal/Non-goals derivation/locking 规则；orchestrated path 需要一个 shared Room Brief contract，同时必须保留 standalone path。
- **当前源码已验证**：现有 standalone planless `review-pr` 只请求/锁定 developer-authored Goal，并把 Non-goals 视为 `none`；本计划保留 direct standalone invocation 与 developer-owned derivation authority，但明确把该 path 加强为 developer-authored Goal + explicit Non-goals 并同时持久化两份 locks，不能再描述为逐字保留旧 locking behavior。
- **当前源码已验证**：`justfile` 提供 repository gates，`just` 与 `cargo nextest` 在当前环境可用；`just test` 包含全量 `cargo nextest` 与 `just maintenance-test`。
- **开发者明确决定**：system/agent/skills 需要把“orchestrator owns process, coding agents own technical work”写成一致身份；hard tool denial 已在 prerequisite harness，实现内容不能冒充 security boundary。
- **开发者明确决定**：workflow 是可动态成长的 SOP。Mermaid 说明当前建议流程、循环和分支，但 agent 根据真实 room state 决定实际下一步，并在 scope 内更新 `.bus/temp`。
- **开发者明确决定（2026-09-16 repair）**：唯一 typed Human approval 是 Room Brief Goal/Non-goals 的 exact confirmation。to-do、participants、models、effort、resources、permissions 与 Human tasks 是 advisory room conversation 和 SOP prose，不是第二个 typed approval 或 workflow state；locked brief 不可变，工作 materially 超出 Goal、Non-goals 或 authority 时，Orchestrator 以 Human-addressed `SendMessage` 请 Human 新建 room 或使用 existing Human-controlled action。
- **开发者明确决定**：workflow complexity、agent count、compute 与 intelligence 增长不改变 harness；content 和 context 可增长。
- **开发者明确决定**：content 必须称 human、orchestrator 与 coding agents 为 first-class room partners；workflow 分配 role/capability，而不是把 agents 描述成由人类操作的软件组件。
- **当前源码已验证（`7bc910f2`，包含 dogfood lifecycle fix `3c96cbf8`）**：Claude background Stop 已成为 explicit `BackgroundPending`；trusted same-session continuation 会 rebind 并等待 later final；exact confirmed recovery 只在 named Request 仍由已确认 Idle agent 持有时标记 `Abandoned`。这些都是 content 必须教 Orchestrator 正确解释的 lifecycle facts/capability settlements，不能成为 runtime 自动推进规则。
- **开发者明确决定（2026-09-16 repair）**：三份 built-in SOP 作为 embedded production bundle 的 named `reference` entries 交付，并由 `ReadContent` 读取；动态或适配 SOP 只走 `PersistWorkflowDraft` 与 `ReadWorkflowDraft`；`PromoteWorkflowDraft` 把 developer-approved revision 发布到 `.bus/standard`，供后续 canonical incorporation，当前 model 不 raw-read `.bus/standard`。Capability token 只是 model 可选择的 typed call 名称，不把 SOP 变成 executable graph，也不授权 checker 或 runtime 选择调用顺序。

```mermaid
flowchart LR
    H[Closed prerequisite harness] --> C[injected system + agent + index]
    C -->|ProposeRoomBrief| RB[Room Brief proposal]
    RB -->|exact Human confirmation| G[baseline grants]
    G --> RC[ReadContent: skills, template, built-in SOPs]
    RC --> SOP[Markdown + Mermaid SOP]
    SOP -->|adapt within locked scope| D[PersistWorkflowDraft and ReadWorkflowDraft]
    D --> SOP
    SOP --> EW[execute-workflow judgment]
    EW -->|SendMessage and other closed calls| A[Coding agents and Human]
    A -->|WorkSettlement facts and replies| EW
```

## 参考资料

- `plans/2026-09-15-room-orchestrator-agent-led-harness.md`（canonical prerequisite；final closed Orchestrator surface、Room Brief bootstrap/confirmation authority，以及删除 synthetic acceptance driver 与 artifact/outline queries 的归档决策）
- `plans/2026-09-14-room-orchestrator-content.md`、`plans/2026-09-14-room-orchestrator-harness.md`（legacy plans，迁移后只保留 abandoned pointer）
- `docs/templates/plan-template.md`（完整读取；canonical plan contract）
- `docs/guides/consumer-fallout-format.md`（完整读取；consumer inventory interpretation）
- `skills/AGENTS.md`（完整读取；canonical skill ownership and Bus gate conventions）
- `skills/skill-architecture.md`（完整读取；execution/principle/mechanic/format layering）
- `skills/create-plan/SKILL.md`、`skills/review-plan/SKILL.md`、`skills/execute-plan/SKILL.md`、`skills/pr/SKILL.md`、`skills/review-pr/SKILL.md`、`skills/address-review-comments/SKILL.md`（current coding-agent goal and workflow contracts）
- `docs/guides/review-response-guide.md`（完整读取；review findings remain evidence-bearing claims and technical disposition stays with coding-agent author）
- `docs/guides/room-orchestrator-content-interface-format.md`、`docs/guides/room-brief-projection-format.md`（完整读取；content manifest/packing/read 与 trusted assignment verifier result SOT）
- `build.rs`、`src/bus/orchestrator/content.rs`、`src/bus/orchestrator/tests.rs`（当前 `test-agent-led` packing、flat-path/caps/digest 校验与 production typed-unavailable 分支）
- `src/bus/orchestrator/types.rs`、`src/bus/orchestrator/registry.rs`、`src/bus/runtime_orchestrator.rs`、`src/bus/workflow_drafts.rs`（closed query/operation vocabulary、capability gate、request content injection、draft/promotion semantics）
- `src/main.rs`、`src/bus/entry.rs`、`docs/how-to-bus-cli.md`（`--bus assignment verify --frame FRAME` 入口与 typed JSON result）
- `.gitignore`、`justfile`（`/.bus/` ignored runtime workspace；`test-one`、`maintenance-test`、`test`、`lint` 与 `build` gates）
- `scripts/test_skill_migration_contract.py`、`scripts/test_sanitize_review_severity.py`、`scripts/test_bus_dev_acceptance.py`（完整读取；adjacent deterministic Python test patterns and existing no-severity/link contracts）

> **重要**：
> - 使用官方库 / 框架 / SDK 文档，并在本节记录实际读取的链接
> - 如果任务涉及现有模块，必须包含适用的 `AGENTS.md`；Bus 当前只有 `skills/` 与 vendored libghostty-vt 树维护局部 `AGENTS.md`
> - 如果任务跨模块或触及仓库级契约，必须包含对应的根文档、`docs/` 文档或 `skills/skill-architecture.md`
> - 如果任务涉及第三方 SDK，应包含以下链接：
>   - SDK 集成文档
>   - 如何启用 XX 功能的文档
> - 创建计划时先阅读所有适用 `AGENTS.md`，再由现有模块和调用点确定职责、依赖方向、公开 API、测试与验收入口
> - 新增的模块、文件、类型、函数命名应遵循相邻代码与对应模块 `AGENTS.md` 中体现的命名约定
> - 如果计划包含测试，应参考相邻 Rust `#[cfg(test)]`、模块测试、`scripts/test_*.py` 或集成资源测试的既有模式
> - 如果计划涉及日志输出或运行时不变量，必须读取实际拥有该行为的 Rust 模块和现有测试
> - 如果计划涉及跨平台行为，必须说明 Unix/macOS 与 Windows 各自的可用验证入口
> - 如果此任务基于另一个任务，或未来任务依赖此任务，应包含相关任务计划文档的路径
> - 如果找不到或未提供所有相关文档，AI 应停止计划生成并通知开发者

## 需要决策的事项

**当前计划完整程度**：100%

无待确认事项。

> **注意**：初始生成计划时，此完整程度应留空。随着开发者做出更多决策，AI 应更新此百分比。

> **重要**：
> - 只有当计划完整程度达到或超过 95%（AI 有 95%+ 把握能完成任务）时，开发者才可以执行计划
> - 任何不清楚、缺失或可以用不同解决方案实现的事项都应列在此部分
> - AI 不应猜测，应在计划执行前始终询问相关信息
> - 如果 AI 无法推断出可用选项，可以提出开放性问题
> - 每个决策项可以有多个选项（不限于两个），根据实际情况列出所有可行的选择
> - 每个选项的描述应列出优缺点（pros and cons）
> - 在做出大的方向性决策后，AI 应继续提出后续问题并更新此部分
> - 开发者做出决策后，应更新此完整程度百分比
> - 新增开放决策后，必须把 **状态** 改回 `create-plan-in-progress`。此回退由开发者/计划作者在新增该决策时同步完成；`/address-review-comments` 等审查响应工具不代做此回退

## 已归档的决策

1. **Process authority**
   - **选项**：content 描述 fixed runtime；content 只在 failure 时唤醒 agent；content 指示 agent 持续拥有流程。
   - **选择**：第三项。`execute-workflow` 在每次 room event 后由 agent 判断下一步，所有 dispatch/wait/adaptation/recovery 都通过 tool call 明确发生。
   - **依据**：开发者明确选择 B 并要求彻底移除 script+agent design。

2. **Mermaid semantics**
   - **选项**：可执行 DSL；装饰图；SOP 的 normative visual map。
   - **选择**：第三项。图必须与 prose 对齐并可表达循环/分支/并行/人类步骤，但现实状态和 agent judgment 决定执行；agent 可在 `.bus/temp` 修订图与 prose。
   - **依据**：开发者选择 Mermaid-in-Markdown，同时强调 workflow 是 SOP、不是 script。

3. **Production content ownership**
   - **选项**：runtime hard-code；repo file shadow；fixed versioned content bundle。
   - **选择**：第三项。System、agent、两个 skills、workflow template 与三份 built-in SOP 全部作为 embedded production bundle 的 named entries 交付，Orchestrator 只以 `ReadContent` 读取。Adapted SOP 只存在于 Bus-derived `.bus/temp` draft；`PromoteWorkflowDraft` 把 developer-approved revision 写到 `.bus/standard`，作为后续 canonical incorporation 的输入，当前 model 不读取该目标。本计划不交付 checked-in `.bus/` 文件，因为 `/.bus/` 是 git-ignored runtime workspace。
   - **依据**：content 与 harness 分计划、避免 repo prompt injection shadow production identity；前置计划删除 artifact query 后，开发者 2026-09-16 repair 决定以 embedded reference 承载 standard SOP，而不新增 query。

4. **Technical boundary wording**
   - **选项**：orchestrator 可直接分析代码；只用 prompt 禁止；prompt + harness capability denial。
   - **选择**：第三项。Content 要求 route technical questions, compare process evidence, ask agents for recommendations；不能声称 prompt 能阻止越权。
   - **依据**：开发者要求 harness-level constraint，content 只加强角色一致性。

5. **Standard convergence**
   - **选项**：单 reviewer；同 family reviewers；independent mixed-family lanes + separate author + repeated convergence。
   - **选择**：第三项。每轮 lanes 不读 peers；有 finding 时收集 reviewers 与 author recommendations，把完整集合交给 author 做 best-of-N-style resolution；无法 cleanly resolve 时才找 human。全部 canonical lanes 对同一 plan hash Ready 后，Orchestrator 必须显式授权 separate author 执行 hash-bound state finalization；reviewer 和 runtime 都不从 verdict 自动推进。
   - **依据**：开发者对 mixed-model review 和实际 Bus dogfood 的明确流程。

6. **Dynamic scope and approval**
   - **选项**：每次 SOP 改动重批；批准后任意扩 scope；locked boundary 内动态修订。
   - **选择**：第三项。唯一 typed approval 是 Human 对 `ProposeRoomBrief` proposal 的 exact Goal/Non-goals confirmation；确认前 Orchestrator 只能提出 brief，因此形成首个 proposal 所需的全部指导位于注入的 `agent.md`（及适用的 system/index）。确认后 routine adaptation、attempt/lesson 增长、to-do/participant/model/effort/resource/permission 讨论与 `.bus/temp` 更新都不重批，只作为 advisory room conversation 与 SOP prose。Locked brief 不可变；Goal/Non-goals、authority、destructive/publish 或 material budget/resource 超出时，Orchestrator 以 Human-addressed `SendMessage` 请 Human 新建 room 或使用 existing Human-controlled action，不新增 update、unlock 或 re-proposal operation。
   - **依据**：开发者需要 agent 从 entropy 中恢复，同时保留 human authority；前置 runtime 只在 unlocked room 允许 bootstrap `ProposeRoomBrief`，并在首次 exact confirmation 时激活 baseline grants（开发者 2026-09-16 repair）。

7. **Participant language and authority**
   - **选项**：human controller + agent components；所有参与者完全相同权限；first-class partners + role-specific capabilities。
   - **选择**：第三项。System、agent、skills、template 与 SOP 都使用 participant/partner/assignment/evidence vocabulary；human approval 与 orchestrator process ownership 是能力差异，不是否定 agent 主体性。
   - **依据**：开发者明确指出这是 Bus 的核心模型，并强调 orchestrator agent 拥有很大权力。

8. **Production content interface boundary**
   - **选项**：本计划同时实现 build/runtime assembly；只交付 static content；消费 prerequisite-owned generic interface。
   - **选择**：第三项。Prerequisite 的 `ROOM_AGENT_CONTENT_INTERFACE_V1` 保持 schema、caps、digest、closed selector、model-authored read 与 prompt layering 的定义者；本计划只在 `build.rs`、`src/bus/orchestrator/content.rs` 与 `src/bus/orchestrator/tests.rs` 做最小 production activation：`src/bus/orchestrator/content/production/manifest.json` 存在时复用同一校验打包，并由 `production` selector 返回；缺失时仍为 `ProductionUnavailable`，非法时 build fail closed。Manifest 文件名是 `manifest.json`，全部 entry 都是 selector 目录内 flat `.md` 文件。不改 provider、journal、tool registry、permission、persistence、TUI、control 或 safety surface，也不增加 production smoke driver。
   - **依据**：四份完整独立 recommendation 均选择 A；Task 1 generation 1 blocker 证明当前 build/loader 永不打包 production tree，开发者 2026-09-16 repair 授权这项最小 activation。

9. **Trusted Room Brief carrier and precedence**
   - **选项**：content-owned in-band block；延期六个 skill branches；harness-produced versioned trusted assignment projection。
   - **选择**：第三项。Prerequisite Worker 在已授权 coding-agent Request delivery 中产生 `TRUSTED_ROOM_ASSIGNMENT_V1`，prerequisite production verifier 独占 directory、immutable record、active pointer、endpoint/token、frame、current Request、resume/replace provenance 的信任判定。本计划唯一 shared context adapter 与六个 skill branches 不读取或重新验证 harness storage，也不信任 in-band lookalike：outer frame 与四项 Bus discovery variables 全部不存在时返回 distinct local `NotInBusRoom`，不调用 verifier 并保留 standalone path；任一 Bus-intent signal 存在但 frame 缺失或 discovery 不完整时直接 blocker，没有 standalone fallback；frame 与完整 discovery 同时存在时才调用 production verifier。Verifier `verified` 授权 orchestrated branch；`invalid`、malformed output、timeout、executable unavailable 或 nonzero exit 都是 blocker。CLI 要求 `--frame`，因此 adapter 不声称可达的 verifier-`absent` standalone result，出现该结果按 malformed 处理。Plan Goal/Non-goals 与 `.locked-goal`/`.locked-non-goals` 必须 exact match，否则返回 blocker evidence 给 Orchestrator，绝不覆盖、fallback 或自动选择 recovery。
   - **依据**：四份完整独立 recommendation 均选择 A；only harness can bind durable room/work/request、author/recipient and approved brief revision without turning prompt prose into a trust root；`docs/guides/room-brief-projection-format.md` 规定 CLI 缺失 discovery 报 `invalid`、从不返回 `absent`。

10. **Closed prerequisite surface reconciliation**
   - **选项**：恢复已删除 queries/operations 与 acceptance driver；在 content 中为旧 token 保留 alias；把 content、checker 与 gate 全部改写到 current closed surface。
   - **选择**：第三项。删除全部 synthetic acceptance driver、counter 与 `justfile` replay 要求，不以其它 harness 替代；删除 static expected-model-choice scenarios 与 semantic wording classifier，checker 只校验 objective entry set、structure、removed-token vocabulary、ownership 与 safety facts。旧概念只映射为 model judgment：steering 或 pivot 是新的 model-authored `SendMessage` Request；technical/codebase 问题以 `SendMessage` 委派；Human help 是 Human-addressed `SendMessage`；create/delete/replace agent 由 room message 请 Human 使用 existing Bus actions；evidence 来自 `InspectWork` 与 `WorkSettlement`；Goal/Non-goals 走 `ProposeRoomBrief` 与 exact Human confirmation；attempt、lesson、to-do、participant、model、effort、resource、permission 与 Human task 只是 advisory SOP prose。
   - **依据**：Task 1 generation 1 blocker B1–B6 与开发者 2026-09-16 repair 决定；前置计划归档决策删除 Task 3 driver、artifact/outline queries 与 lifecycle/steering operations。Fixtures 不执行模型，也不证明 semantic orchestration quality；live provider calls 保持为零，Human 提供的 DeepSeek key 只用于后续人工端到端检查。

## 大小

**大小**：XL

> **大小说明**：
> - `大小` 字段只允许填写一个等级 token：`XS`、`S`、`M`、`L`、`XL`、`XXL` 或 `XXXL`。
> - 禁止在 `大小` 字段后追加括号说明、scope、文件数量、行数、抽象数量或工作清单。例如应写 `**大小**：S`，不要写 `**大小**：S（client-only：...；约 6-7 个文件；新增 1 个 helper）`。
> - scope、涉及文件、抽象与工作内容应写在「当前状态分析」、「需要修改/添加的文件」和「实施步骤」中，而不是塞进 `大小` 字段。
> - `XS`: 超小任务（代码变更行数 < 50，涉及文件数 < 3，新增抽象数 0）
> - `S`: 小任务（代码变更行数 50-200，涉及文件数 3-5，新增抽象数 0-1）
> - `M`: 中等任务（代码变更行数 200-500，涉及文件数 5-10，新增抽象数 1-3）
> - `L`: 大任务（代码变更行数 500-1000，涉及文件数 10-20，新增抽象数 3-5）
> - `XL`: 超大任务（代码变更行数 > 1000，涉及文件数 > 20，新增抽象数 > 5）
> - `XXL`: 特大任务（代码变更行数 > 3000，涉及文件数 > 50，跨多个模块的架构变更）— 适合由 AI Agent 主导执行、有完整单元测试覆盖的场景
> - `XXXL`: 巨型任务（代码变更行数 > 5000，涉及文件数 > 100，系统级架构重构）— 仅适用于 AI Agent 全程执行 + 完整自动化测试套件可兜底的 yolo 场景
>
> **机械性变更降级规则**：
> 上述大小阈值衡量的是**设计/审查复杂度**，不是 diff 行数或文件数。如果绝大部分变更是纯机械操作——每处都遵循同一条可机械验证的规则、不涉及业务逻辑或设计判断——应将大小**至少降一档，必要时大幅降级**（例如：100 文件的方法重命名实际复杂度可能只是 XS/S，因为审查者抽样 3-5 处确认替换规则一致即可，不需要逐文件思考）。
>
> 典型机械性变更：
> - 重命名公开方法 / 类 / 字段，导致全仓库 N 个调用点跟改
> - 修改某个广泛使用的函数签名（增删参数、改返回类型），所有调用点机械跟改
> - 模块拆分 / 重组，大量文件移动 + import 路径调整
> - 批量修复新增 lint 规则触发的全仓库违规
> - 按 codemod 规则批量替换 API（旧 API → 新 API 迁移）
> - 统一 import 顺序 / 路径 / 别名
> - 目录重组、按规范批量重命名文件
> - 给已有未标注的代码批量加类型注解
>
> 判断准则：
> - 审查者是否需要逐文件思考？如果只需抽样核对「是否都按同一规则改」，就是机械性变更
> - 仍按原始复杂度计大小的部分：触发机械变更的「源头」本身（新增的 lint 规则、codemod 脚本、新 API 接口、新签名的方法声明）不参与降级

> **⚠️ 不可修改**：以下规则部分必须包含在每个计划文档中，AI 和开发者不得修改此部分。

<!-- 计划生成规则：开始 - 此部分不可修改 -->
## 计划生成规则

- **作者**字段必须填写 GitHub 用户名（通过 `gh api user -q .login` 获取），不得使用 "claude_code"、"AI" 等非人类标识符。此字段用于追踪计划质量归属。
- **分支**字段必须是当前具名分支。默认使用 feature 分支；只有开发者明确要求时才允许直接在 `master` 落盘。
- 一份计划只有一个目标；大小不等于多目的。
- AI 生成的计划文档必须包含本模板中的所有部分；标记为「（如适用）」的部分是可选的，开发者可以选择主动删除。
- 所有 `> **重要**：` 块必须从模板中复制，不得修改或省略。
- 开放决策存在时，保持 `create-plan-in-progress`，不生成文件契约、任务图或测试计划。
- **当前计划完整程度**初始应留空，AI 不得自动填写百分比；随着开发者做出更多决策，AI 应更新此百分比。
- **大小**、**需要修改/添加的文件**、**错误跟踪**、**断言检测**、**实施步骤**和**测试计划**初始应留空，仅当以下条件全部满足后才生成和更新内容：
  - 「参考资料」已完整
  - 「需要决策的事项」中无未解决的问题
  - 当前计划完整程度达到或超过 95%
- 决策清空后，正文完整程度必须达到至少 95%，任务通常为 1–5 个粗粒度单元。
- 开发者做出决策后：
  - 必须将决策保存到「已归档的决策」
  - 如果上述各生成部分不为空，必须同步更新它们以反映新的决策
- 当以下条件全部满足时，AI 必须自动将**当前计划完整程度**更新为 100%：
  - 「需要决策的事项」中没有未解决的问题（所有决策已归档）
  - 所有必需的生成部分均已填写，并与已归档决策保持一致
- `拥有文件` 是排他写入边界。不同任务 owner 不得重叠；明确文件必须由一个且仅一个 owner 覆盖。
- `consumes` 必须同时出现在 `blocked-by`；任务依赖必须是无环图。
- POC 计划只承诺验证目标所需的最小闭环。生产打包、性能指标、网络、云服务等只有在目标明确要求时才能进入范围。
<!-- 计划生成规则：结束 -->

## 需要修改/添加的文件

列出所有将被添加或修改的文件，并提供高级概念代码片段和关键实现细节。

> **重要**：
> - 每个有行为变化的 source 文件都必须附一段简洁的**高级概念代码片段**，用于锁定公开签名、关键类型或控制流；片段不含 import、不标行数，也不代替完整实现。
> - **不要在代码片段中包含 import 语句**——import 是实现细节，不传达设计意图。
> - **路径迁移只说明一次，不要逐处列举**：当某个 Rust 模块或公开符号迁移导致消费方路径机械跟改，只需说明所有引用都指向新路径，不把每个 `use` 改动逐条列出。
> - 对于关键的实现细节，可以添加代码片段，但不需要完整实现；也可以在片段中用注释说明需要在何处添加或修改什么。
> - **禁止标注代码行数估算**（如 `~20 LOC`、`约 50 行`、`+30/-10`）：行数对执行 agent 是噪音，文件职责和高级概念片段才是有用信息。
> - `大小` 字段也不能承载文件数量、抽象数量或工作清单；这些内容应由本节逐文件说明。
> - 需要说明新组件如何融入现有架构时使用 mermaid 绘图。
> - 此部分应详尽，包含所有将被添加或修改的文件，并写明 repo-relative 路径。
> - 测试文件可在本节或「测试计划」中用 `describe` / case 骨架表达，但必须能对应到计划中的行为契约。
> - **例外**：以下类型的文件无需在此部分逐一列出代码片段，可直接修改：
>   - 包 / 构建配置文件（如 `Cargo.toml`、`justfile`、`.github/workflows/*.yml` 等）
>   - lock 文件（如 `Cargo.lock`）
>   - 文档文件（如 `.md`、`.txt`）
>   - 仅涉及 import 语句变更的文件
> - 上述例外只豁免本节逐文件代码片段，不豁免任务 ownership、验收闸门或模块 SOT 同步。计划已经明确点名的文件与测试路径必须进入本节文件契约，并被一个且仅一个任务 owner containment 覆盖；合理 deviation 可在稳定 owner 内新增未预知文件。
> - 改变 skill 架构或公开工作流契约时，`skills/AGENTS.md`、`skills/skill-architecture.md` 与受影响的共享 guide 必须进入同一 owner 与 gate。

- **修改文件**：`build.rs`
  - **用途**：把现有 fixed-tree content validation/packing 从单一 `test-agent-led` 泛化到 closed selector 列表。`test-agent-led` 仍必需；`production` 在 `src/bus/orchestrator/content/production/manifest.json` 存在时复用同一 interface/version/compatibility/flat-path/UTF-8/per-file cap/total cap/SHA-256/name 校验并打包，缺失时写出 absent marker，非法时 build fail closed。不接受 env、path 或 repository shadow selector，不改其余 build 步骤。
  ```rust
  fn main() {
      // Existing build steps stay unchanged.
      pack_content_selector(&manifest_dir, "test-agent-led", SelectorPresence::Required);
      pack_content_selector(&manifest_dir, "production", SelectorPresence::WhenManifestExists);
  }

  fn pack_content_selector(manifest_dir: &Path, selector: &str, presence: SelectorPresence) {
      // Reuses validate_content_entry and the existing packed shape. An absent optional selector
      // writes `null` to OUT_DIR/bus-<selector>-content.json so the loader reports it unavailable.
  }
  ```
- **修改文件**：`src/bus/orchestrator/content.rs`
  - **用途**：`ContentLoader` 同时解析 `test-agent-led` 与可选 `production` packed output；`production` selector 在 bundle 存在且 compatible 时返回该 bundle，缺失时返回 `ProductionUnavailable`。现有 constructor 名称保留，因为 `src/bus/runtime_orchestrator.rs` 已用它服务两个 selector 且不在本计划 owner 内；`ContentBundle::read` 的 kind/name pairing 不变。
  ```rust
  pub(crate) struct ContentLoader {
      test: ContentBundle,
      production: Option<ContentBundle>,
  }

  impl ContentLoader {
      pub(crate) fn load(&self, selector: ContentSelector) -> Result<ContentBundle, ContentError> {
          match selector {
              ContentSelector::TestAgentLed => compatible(&self.test).cloned(),
              ContentSelector::Production => match &self.production {
                  Some(bundle) => compatible(bundle).cloned(),
                  None => Err(ContentError::ProductionUnavailable),
              },
          }
      }
  }
  ```
- **修改文件**：`src/bus/orchestrator/tests.rs`
  - **用途**：以 `room_orchestrator_core_content_` 前缀替换“production 永远 unavailable”断言：packed production bundle 经 `production` selector 加载，interface/compatibility/version/digest 有效；`system`、`agent`、`index`、`skill/create-workflow`、`skill/execute-workflow`、`reference/workflow-template`、`reference/sop-review-plan`、`reference/sop-review-pr` 与 `reference/sop-execute-plan` 均可经 `ContentBundle::read` 读取，未注册 entry 返回 `UnknownEntry`；test-only 构造的无 production loader 返回 `ProductionUnavailable`；`test-agent-led` 行为不变。
- **新文件**：`src/bus/orchestrator/content/production/manifest.json`
  - **用途**：`ROOM_AGENT_CONTENT_INTERFACE_V1` 的 production instance：`content_version` 与 `compatibility` 为 1，required `system`/`agent`，`skills` 为 `create-workflow`、`execute-workflow`，`references` 为 `workflow-template`、`sop-review-plan`、`sop-review-pr`、`sop-execute-plan`；每项是 flat `.md` path 与 SHA-256，合计八个 concrete assets 并满足 per-file/total caps。不声明 tool、capability、mode、transition 或 control logic。
- **新文件**：`src/bus/orchestrator/content/production/system.md`
  - **用途**：建立 harness-level identity：Orchestrator 是 first-class room participant 与 process owner；coding agents 和 Human 是 partners；所有 next-step decisions 属于 Orchestrator model。
  - **关键修改**：明确 no technical problem solving、evidence-first routing、no self-approval、no hidden reasoning exposure；承认 hard security 来自 harness capabilities，而不是 prompt。
- **新文件**：`src/bus/orchestrator/content/production/agent.md`
  - **用途**：每次 request 都注入的持续运行方式，并包含形成首个 Room Brief proposal 所需的全部指导，因为确认前不能 `ReadContent` 或 `PersistWorkflowDraft`。
  - **关键修改**：确认前只理解 Human requirements，并以 exact expected revision 与 nonblank Goal/Non-goals 选择 `ProposeRoomBrief`，然后等待 Human exact confirmation；确认后才从 injected index 选择 exact entry，并以 `ReadContent` 加载 skills、template 与 built-in SOPs。以 `InspectWork` 返回的 `WorkSettlement`、`WaitForChange`、model-selected `ReadAgent`（`VisibleViewport` 或 positive-N `RecentTail`）与 `ReadWorkflowDraft` 获取 bounded facts，不相信 visual Idle。区分 `BackgroundPending`、trusted continuation、Request/operation settlement、reply evidence 与 higher-level outcome。Assignment、pivot、technical question 与 Human help 都是 model-authored `SendMessage`；agent create/delete/replace 通过 room message 请 Human 使用 existing Bus action；permission 先以 `ObservePermissionPrompt` 获得 single-use fingerprint，再判断是否 `ApprovePermissionOnce`；wedged current Request 只以 `AbandonIdleRequest` 恢复；shared resource 使用 `AcquireResource` 与 `ReleaseResource`。Locked brief 不可变，material expansion 请 Human 新建 room 或采取 Human-controlled action。
- **新文件**：`src/bus/orchestrator/content/production/create-workflow.md`
  - **用途**：确认后读取的 skill：从 index 选择 built-in SOP reference 并以 `ReadContent` 读取；只有 model 决定适配时，才以 pathless `PersistWorkflowDraft` 创建或修订 Bus-derived `.bus/temp` draft，并以 receipt draft id 调用 `ReadWorkflowDraft`。
  - **关键修改**：Built-in SOP 可直接执行，不为读取而 materialize draft，也不提交 path 或 filename；以 Human-addressed `SendMessage` 分享 advisory to-do、participants、models、effort、resource 与 permission needs，不创建第二个 typed approval。Standard publication 需要 Human review surface 对 immutable draft revision/content、current standard base 与 exact diff 签发 developer approval；其后 Human 或 Orchestrator 才可选择 approval-bound pathless `PromoteWorkflowDraft`，runtime 只重验并结算。
- **新文件**：`src/bus/orchestrator/content/production/execute-workflow.md`
  - **用途**：让 Orchestrator 在每个 room event 后读取当前 SOP、durable facts 与 evidence，自主决定并执行下一步。
  - **关键修改**：Current SOP reference 只记录 `ReadContent` reference name 或 `ReadWorkflowDraft` draft id；unknown、missing 或 mismatched 结果只返回事实，model 可重新 inspect/select 或请 Human，runtime/helper 不 fallback、不复制、不选择 recovery。每个 closed query/operation call 都是 model judgment；typed stale/denied/uncertain facts 只触发重新判断。Claude `BackgroundPending` 不结算、trusted same-session continuation rebind、ordinary final 只结算 Request、post-settlement activity 不宣告 work complete、exact idle-request recovery 只暴露 `Abandoned`；不得等待 harness 自动推进，scope 内 revision 只通过 `PersistWorkflowDraft`，publication 只通过 developer-approved `PromoteWorkflowDraft`。
- **新文件**：`src/bus/orchestrator/content/production/workflow-template.md`
  - **用途**：作为 `plan-template.md` 的 workflow 对应物，提供 Markdown sections 与 exactly one Mermaid flowchart 的完整可复制 SOP skeleton。
  - **关键修改**：包含 intent、inputs、participants and capabilities、Room Brief boundary、current SOP source、success evidence、settlement evidence、failure signals、adaptation and recovery authority、resource leases、attempt ledger、Human tasks、stop and escalation、revision history sections；current SOP source 只能是 `ReadContent` reference name 或 `ReadWorkflowDraft` draft id，不含 raw path。Capability slots 只写 closed query/operation token；attempt ledger、to-do 与 Human tasks 是 advisory prose，不是 typed state；图与 prose 对齐，但图不具可执行语义。
- **新文件**：`src/bus/orchestrator/content/production/{sop-review-plan.md,sop-review-pr.md}`
  - **用途**：描述三条 mixed-family independent reviewer lanes、separate author、循环至全部 Ready 的 built-in SOP，由 `ReadContent` 读取。
  - **关键修改**：Lane 顺序与隔离由 Orchestrator 以 `SendMessage` assignment 维护；有 finding 时向三名 reviewers 与 author 收集建议，把四份完整建议交给 author 做 best-of-N-style resolution；无法 cleanly resolve 时才以 Human-addressed `SendMessage` 求助。All-Ready 只是事实；Orchestrator 另行以明确 assignment 授权 separate author 执行 exact plan-hash/lane/round-bound finalization，reviewer/runtime 不自动写 `review-plan-complete`。
- **新文件**：`src/bus/orchestrator/content/production/sop-execute-plan.md`
  - **用途**：由 room Orchestrator 逐次选择 reviewed plan 中可执行的 bounded implementation、task-review、remediation 与 finalization assignment，coding agents 使用 native technical tools 完成被委派的细节，再进入 mixed-model independent review/address cycle。
  - **关键修改**：Orchestrated branch 不启动或跟随 standalone `execute-plan` Python progression driver；deterministic helpers 只验证 Orchestrator 已选择的 task/owner/hash/evidence/operation。Orchestrator 避免 simultaneous uncoordinated writers、按任务复杂度分配 effort，并在每次 returned fact 或 reply 后自主决定 wait、next assignment、repair 或 recovery。
- **新文件**：`scripts/orchestrator_content_check.py`
  - **用途**：只做 objective static checks：production manifest 的 required entry names 与 flat files 存在；template 具备 required sections 且恰有一个 Mermaid flowchart；每份 built-in SOP 恰有一个 Mermaid flowchart；production content、六个 canonical skills 与 `docs/guides/orchestrated-room-brief.md` 不含前置计划删除的 query/operation/handle token（完整列表以前置计划归档决策为准）或其 snake-case tool 名。Schema、digest 与 caps 由 `build.rs` 在构建时校验，不在 Python 重复；checker 不解释图的控制流、不评估 wording 语义、不执行模型。
  ```python
  def validate_repository(repo: Path) -> tuple[Diagnostic, ...]:
      return (
          *validate_production_entry_set(repo),
          *validate_workflow_template_structure(repo),
          *validate_single_mermaid_flowchart_per_sop(repo),
          *validate_no_removed_capability_tokens(repo),
      )
  ```
- **新文件**：`scripts/test_orchestrator_content_check.py`
  - **用途**：checked-in production tree 通过；temp-copy 变体证明缺 required entry、缺 template section、零或多个 Mermaid flowchart、任一 removed token（CamelCase 或 snake-case）各自产生指明文件的 diagnostic；不 subprocess 任何 driver，也不调用 provider。
- **修改文件**：`skills/AGENTS.md`
  - **用途**：记录 shared Room Brief、participant 与 handoff precedence 以及 canonical skill ownership，引用 `docs/guides/orchestrated-room-brief.md`，不复制 producer/verifier format 或各 skill 的 authority prose。
- **修改文件**：`skills/skill-architecture.md`
  - **用途**：把 shared room-assignment consumer 与六个 skill branches 放入现有 execution/principle/mechanic/format layering。
- **新文件**：`docs/guides/orchestrated-room-brief.md`
  - **用途**：消费 `docs/guides/room-brief-projection-format.md` 的 verifier typed-result contract，定义唯一 branch mapping：no outer frame 且 `BUS_BINARY`、`BUS_TRUSTED_ASSIGNMENT_DIR`、`BUS_TRUSTED_ASSIGNMENT_ENDPOINT`、`BUS_TRUSTED_ASSIGNMENT_TOKEN` 全部不存在时是 local `NotInBusRoom` standalone；任一 Bus-intent signal 缺 frame 或 discovery 不完整时是 blocker；frame 与完整 discovery 同时存在时调用 verifier，`verified` 授权 orchestrated branch，其余结果一律 blocker。Orchestrator 以 `ProposeRoomBrief` 拥有 room-level brief proposal 并由 Human 确认，coding-agent skills 只消费 locked Goal/Non-goals。
- **新文件**：`cli_extensions/room_assignment_context.py`
  - **用途**：六个 skill 共用的唯一 `TRUSTED_ROOM_ASSIGNMENT_V1` discovery/verifier-result consumer。它只检查 outer-frame presence 与四项 discovery variable presence，并只通过 `BUS_BINARY` 以 `--bus assignment verify --frame <outer-frame>` 调用 production verifier；decode `verified | invalid`，把 `verified` payload 变成 shared Goal/Non-goals/participant context。`invalid`、CLI 不可达的 `absent`、malformed output、executable unavailable、timeout 与 nonzero exit 都是 blocker。它不打开 `BUS_TRUSTED_ASSIGNMENT_DIR`、不读取 immutable record/active pointer/token、不自行比较 identity/revision/digest，也不实现第二套 trust validator。
  ```python
  def load_assignment_context(frame: str | None, env: Mapping[str, str]) -> AssignmentContext:
      present = present_bus_discovery_variables(env)
      if frame is None and not present:
          return StandaloneContext(origin="NotInBusRoom")
      if frame is None or present != REQUIRED_BUS_DISCOVERY_VARIABLES:
          return BlockedContext(reason="incomplete-bus-intent")
      return map_verifier_result(run_production_verifier(frame, env))
  ```
- **新文件**：`scripts/test_room_assignment_context.py`
  - **用途**：以 fake verifier executable 覆盖 no-frame/no-discovery `NotInBusRoom`（未调用 verifier）、frame-without-discovery、discovery-without-frame 与每种 partial discovery（均为 blocker 且未调用 verifier）、complete input 的 exact argv、executable unavailable、timeout、nonzero exit、`verified` mapping、`invalid`、`absent` 与 malformed output blocker，以及 Goal/Non-goals precedence 与 no direct assignment-storage reads。Forged body、identity/revision/digest、token/frame 与 resume/replace truth cases 仍归前置 verifier tests，不在这里重写。
- **修改文件**：`skills/{create-plan,review-plan,execute-plan,pr,review-pr,address-review-comments}/SKILL.md`
  - **用途**：shared consumer 返回 `verified` context 时逐字消费 locked Goal/Non-goals 并返回正常 artifact；blocker 结果停止并返回 evidence。只有 local `NotInBusRoom` 保留 direct standalone entry 与 developer-owned derivation authority，但 planless `review-pr` deliberately strengthens context/locking to explicit developer-authored Goal + Non-goals and both locks；artifact/evidence 必须区分 provenance，不能声称逐字保留旧 locking behavior。
  - **关键修改**：create-plan 逐字复制 trusted Goal/Non-goals；plan-bound skills require exact match；orchestrated `execute-plan` bypasses deterministic progression and只完成 Orchestrator 明确选择的 bounded action；Orchestrator 以 `ProposeRoomBrief` 提出 brief 并由 Human 确认，coding skills 不成为第二 goal writer。Reviewer findings 始终是 evidence-bearing claims，不加 severity tags。
- **修改文件**：`skills/pr/scripts/pr_goal_context.py`
  - **用途**：planless orchestrated PR path 从 `verified` payload 同时生成并复用 `.locked-goal` 与 `.locked-non-goals`；standalone `NotInBusRoom` no-plan path 要求 developer-authored goal + explicit non-goal context 并生成相同两份 locks；任何 mismatch 不自动覆盖。
- **修改文件**：`skills/pr/scripts/test_pr_format_check.py`
  - **用途**：继续拥有 PR context production 的 orchestrated 与 standalone both-lock cases，不代替 review-pr consumption tests。
- **修改文件**：`skills/review-pr/scripts/review_round.py`
  - **用途**：直接读取两份 locks，任一 missing/blank fail closed；有 plan 时两者均须与 plan Goal/Non-goals exact match，mismatch 不自动覆盖。
  ```python
  def load_locked_review_context(lane_root: Path, plan: Path | None) -> LockedReviewContext:
      context = read_required_goal_and_non_goals(lane_root)
      if plan is not None:
          require_exact_plan_match(context, plan)
      return context
  ```
- **新文件**：`skills/review-pr/scripts/test_review_round.py`
  - **用途**：direct prologue tests own both-lock parsing、missing/blank/mismatch failures 与 matching plan/no-plan cases，而不是由 `test_pr_format_check.py` 间接代证。
- **新文件**：`skills/address-review-comments/scripts/finalize_plan_review.py`
  - **用途**：提供 separate author 的 exact plan-hash/lane/round-bound `review-plan-complete` finalization operation。Only an explicit Orchestrator assignment invokes it；helper verifies all required canonical Ready artifacts against one current plan hash，then writes only the status field and returns receipt；missing/stale/mixed-hash/finding artifacts fail closed。Reviewer remains read-only，runtime never derives progression from verdict。
- **新文件**：`scripts/test_plan_review_finalization.py`
  - **用途**：覆盖 zero-finding first round、repaired later round、missing/finding/stale/mixed-hash/duplicate-lane failures and reviewer read-only preservation。
- **修改文件**：`docs/guides/review-response-guide.md`
  - **用途**：shared guide records the explicit author-side settlement boundary。
- **新文件**：`scripts/skill_goal_ownership_check.py`
  - **用途**：机械验证 shared guide references、single Room Brief writer、`NotInBusRoom` standalone 与 blocker mapping、derived symlinks、no severity language 和 no duplicated authority。
  ```python
  def validate_goal_ownership(repo: Path) -> tuple[Violation, ...]:
      verify_shared_room_brief_contract(repo)
      verify_orchestrated_and_standalone_skill_paths(repo)
      verify_discovery_links(repo)
      return violations_in_stable_order()
  ```
- **新文件**：`scripts/test_skill_goal_ownership_check.py`
  - **用途**：覆盖六个 skill 的 orchestrated/standalone branches、single writer、participant handoff 与 canonical symlink integrity。
- **修改文件**：`justfile`
  - **用途**：把 content checker、skill ownership checker、room-assignment consumer、plan-review finalizer 与 PR goal-context contract suites 接入 `maintenance-test`，并以 direct `python3 skills/review-pr/scripts/test_review_round.py` line 注册 review-pr prologue suite；不新增 acceptance driver 或 replay target。

明确排除：`Cargo.toml`、`Cargo.lock`、`src/bus/orchestrator/` 中除 `content.rs`、`tests.rs` 与 `content/production/` 外的文件、`src/bus/runtime*.rs`、`src/bus/workflow_drafts.rs`、`src/bus/trusted_assignment.rs`、`src/client/**`、两份前置 format SOT、`.bus/`（git-ignored runtime workspace）以及任何 acceptance driver；本计划只消费前置计划的 generic interfaces，并只做上述最小 production selector activation。

## 实施步骤

以粗粒度、可独立验收的任务图描述执行单元。给模型目标、工具、约束和二元验收闸门，不写操作流程。

> **重要**：
> - 一个任务 = 一段可独立 task acceptance 的完整工作；只因明确并行收益或硬依赖边拆分。天然串行且落在同一文件簇的工作必须合并。
> - `任务<N>` 是计划内全局唯一、不可变的机读 id，从 1 按源码顺序连续递增；名称非空且唯一。
> - `blocked-by` 是唯一调度边；`consumes` 只引用已在 `blocked-by` 中声明的生产方 `任务<N>`；产物名称和契约写在生产方 `produces`。
> - 任意两个任务的 `拥有文件` 必须 containment-aware 两两隔离，依赖边不豁免 overlap；聚合文件和 SOT 只能由一个任务拥有。
> - `拥有文件` 是任务间的排他写入 / 调度边界和最外层写入上限，不是预计 diff 的精确 allowlist。优先选择可容纳合理 deviation 的稳定模块 / 子系统边界；目录 owner 可以包含文件清单未逐项点名的 descendant，但不授权与任务目标无关的改动。
> - 本计划明确点名的新增 / 修改 / 删除文件与具体测试文件必须进入文件契约，并被一个且仅一个任务 owner containment 覆盖；配置、文档、仅 import 等文件清单例外不豁免 ownership / gate。改变公开契约时，对应的 Bus 文档或 skill SOT 必须进入同一 owner 与 gate。
> - 对迁移 / 删除生产模块运行 `skills/review-plan/scripts/consumer_fallout.py`，逐项核实 Rust `use` / `mod`、其他语言 import/require 与同名测试候选；相关项进入文件契约、owner 和 gate，不相关项写明排除依据。inventory 只辅助发现，不替代语义 review。
> - `目标` 描述做完后是什么样；`工具` 列出必读 SOT、harness 和命令；`约束` 只写必须成立的硬顺序和不变量；`验收闸门` 固定以 `[TASK_LOCAL]` 开头，只给出 `just test-one <filter>`、直接 Cargo/Python/Bun 测试或模块编译等局部正确性命令与二元判定，不把 scoped rustfmt 或完整平台 build 重复写进 task gate。命令必须按当前 `justfile` 与测试 harness 核验；exit 0 但选择零测试或错误 harness 不算通过。
> - 任务块内禁止 `(需要手动操作)`。人工验证只写在「手动测试」section；若人工裁决是后续实施的硬前置，拆成两份计划。
> - 主 agent 只在 task report evidence 有效、`review-task Ready` 且所属 wave isolation 通过时勾选 `**完成**`。主 agent 不逐任务重跑 gate/lint；任务完成复选框是共享工作树进度，不表示已 commit。
> - 自动 gate 完成前所有 actor 必须零 Git：不得 `git add/commit/push`。全部自动 gate 对最终计划 delta 全绿后，`execute-plan` 以 `commit-and-push` 作为最后一步提交并推送到当前具名分支；此后不得再改树。随后交回人工验证、`pr` 与最终 `review-pr`。
> - 全局 EXIT CHECK 固定为 `just lint` → `just test` → 可选 `just build`，不得使用 task/file filter。任何修复改树都从 final lint 重新开始。任务级 Ready 与局部 review 对最终 PR review 不可替代。

### 任务 1：交付 agent-led content、adaptive SOP 与 trusted participant handoff contract

- [ ] **完成**
- **目标**：在一个不可分割的 content/consumer owner 中完成可由 `production` selector 加载的 embedded production bundle（system、agent、create/execute skills、workflow template 与三份 built-in SOP references）、最小 selector activation、objective content checker、`TRUSTED_ROOM_ASSIGNMENT_V1` verifier-result shared consumer、shared Room Brief guide 与六个 canonical coding-agent skill branches；所有内容只使用前置计划的 closed query/operation surface，并一致声明 Orchestrator model 拥有 room progression、Mermaid 只表达可修订 SOP、Human/Orchestrator/Coding Agent 是 explicit-capability partners、technical judgment 留给 coding agents。
- **拥有文件**：`build.rs`、`src/bus/orchestrator/content.rs`、`src/bus/orchestrator/tests.rs`、`src/bus/orchestrator/content/production/`、`skills/AGENTS.md`、`skills/skill-architecture.md`、`skills/create-plan/SKILL.md`、`skills/review-plan/SKILL.md`、`skills/execute-plan/SKILL.md`、`skills/pr/SKILL.md`、`skills/review-pr/SKILL.md`、`skills/address-review-comments/SKILL.md`、`skills/pr/scripts/pr_goal_context.py`、`skills/pr/scripts/test_pr_format_check.py`、`skills/review-pr/scripts/review_round.py`、`skills/review-pr/scripts/test_review_round.py`、`skills/address-review-comments/scripts/finalize_plan_review.py`、`docs/guides/orchestrated-room-brief.md`、`docs/guides/review-response-guide.md`、`cli_extensions/room_assignment_context.py`、`scripts/orchestrator_content_check.py`、`scripts/test_orchestrator_content_check.py`、`scripts/skill_goal_ownership_check.py`、`scripts/test_skill_goal_ownership_check.py`、`scripts/test_room_assignment_context.py`、`scripts/test_plan_review_finalization.py`、`justfile`
- **blocked-by**：无
- **produces**：`AGENT_LED_CONTENT_CONTRACT`（production bundle entries、minimal production selector activation、built-in `ReadContent` 与 temp `ReadWorkflowDraft` SOP read pairing、objective entry/structure/vocabulary checker）与 `ROOM_BRIEF_PARTICIPANT_CONTRACT`（trusted assignment branch mapping、single Room Brief writer、technical ownership、`NotInBusRoom` standalone path、explicit plan-review finalization）
- **consumes**：无
- **工具**：completed prerequisite `ROOM_AGENT_CONTENT_INTERFACE_V1` / `TRUSTED_ROOM_ASSIGNMENT_V1` 与两份当前 `*-format.md` SOT、`--bus assignment verify --frame` typed result、`docs/templates/plan-template.md`、owned skill/docs、`docs/guides/review-response-guide.md`、`scripts/test_skill_migration_contract.py`、`scripts/test_sanitize_review_severity.py`、`just test-one room_orchestrator_core_content_`、`python3 -m unittest scripts.test_orchestrator_content_check scripts.test_skill_goal_ownership_check scripts.test_room_assignment_context scripts.test_plan_review_finalization scripts.test_sanitize_review_severity scripts.test_skill_migration_contract`、`python3 skills/pr/scripts/test_pr_format_check.py`、`python3 skills/review-pr/scripts/test_review_round.py`
- **参考实现**：`docs/templates/plan-template.md` reusable structure；现有 `test-agent-led` bundle 与 `build.rs` packing；existing plan/`.locked-goal` standalone branches and canonical symlink tests；manual Bus review/address cycles；`3c96cbf8` lifecycle tests
- **约束**：开始前前置 contracts 必须已实现并重新审查；Rust 改动只限 `build.rs`、`src/bus/orchestrator/content.rs` 与 `src/bus/orchestrator/tests.rs` 的最小 production selector activation，不改 provider、journal、tool registry、permission、persistence、TUI、control 或 safety surface，也不改 `src/bus/runtime_orchestrator.rs`、Cargo 或 client；production content 不定义 tool/capability，capability token 只使用 closed `InspectWork`、`WaitForChange`、`ReadAgent`、`ObservePermissionPrompt`、`ReadWorkflowDraft`、`ReadContent` 与 `ProposeRoomBrief`、`SendMessage`、`AbandonIdleRequest`、`PersistWorkflowDraft`、`PromoteWorkflowDraft`、`AcquireResource`、`ReleaseResource`、`ApprovePermissionOnce`，不引用或别名任何已删除 token；embedded content（含 built-in SOP）只走 `ReadContent`，temp draft 只走 `ReadWorkflowDraft`，不 raw-read `.bus/standard`、不隐式 materialize、不 helper fallback；形成首个 Room Brief proposal 所需指导必须完整位于注入的 `agent.md`（及适用 system/index），不得假设确认前可 `ReadContent` 或 `PersistWorkflowDraft`；唯一 typed approval 是 Human exact Goal/Non-goals confirmation，advisory 讨论不成为第二审批，locked brief 不可变且不新增 update、unlock 或 re-proposal；manifest 使用 `manifest.json` 与 flat `.md` entry paths 并满足 caps；不交付 checked-in `.bus/` 文件；SOP/Mermaid 不被程序解释或执行，checker 只做 objective checks，不含 expected-model-choice scenarios 或 wording 语义分类；orchestrated `execute-plan` 不调用 deterministic progression driver；shared consumer 只按 `NotInBusRoom`、incomplete Bus-intent blocker 与 verifier call 三个分支工作，`verified` 之外全部 blocker，never reads/revalidates directory、record、active pointer、endpoint/token or authoritative Request facts；review state 只由 Orchestrator 明确授权的 separate-author hash-bound operation 写入；只改 canonical skill copies，derived links 不直接编辑；保留 read-only reviewer、closed-world、triage、landing、no-severity 与 250-line contracts；agents/humans 使用 first-class participant language；不创建 acceptance driver 或 replay target；live provider calls zero
- **验收闸门**：[TASK_LOCAL] `just test-one room_orchestrator_core_content_`、`python3 -m unittest scripts.test_orchestrator_content_check scripts.test_skill_goal_ownership_check scripts.test_room_assignment_context scripts.test_plan_review_finalization scripts.test_sanitize_review_severity scripts.test_skill_migration_contract`、`python3 skills/pr/scripts/test_pr_format_check.py` 与 `python3 skills/review-pr/scripts/test_review_round.py` 均 exit 0 且选择非零测试；Rust tests 证明 packed production bundle 经 “production” selector 加载、八个 required entries 可读、未注册 entry 被拒绝、无 production bundle 时 typed-unavailable 且 “test-agent-led” 不变；Python tests 证明 required entry set、template sections、每份 SOP 单一 Mermaid flowchart、removed-token denial、六个 skill branches、“NotInBusRoom” standalone 不调用 verifier、incomplete Bus-intent blocker 不调用 verifier、complete input 的 exact verifier argv、“verified” Goal/Non-goals/plan-lock mapping、“invalid”/“absent”/malformed/timeout/unavailable/nonzero blocker、direct review-round two-lock parsing/mismatch behavior、no content-owned trust validator、explicit all-Ready finalization、single brief writer、participant handoff 与 derived-link integrity；这些 tests 不执行模型、不证明 semantic orchestration quality，live provider calls 为零

## 测试计划

定义测试策略和方法，以确保实现的功能符合预期并正确处理各种场景。**优先使用自动化测试**，手动测试仅作为最后手段。

> **重要**：
> - **自动化测试优先**：AI agent 可以运行 `just test-one <filter>`、`just test`、`just lint` 和适用的 Python/Bun 测试，因此脚本可执行的验证不归类为手动测试。
> - **手动测试仅限于最后手段**：只有终端交互、视觉流畅度、真实 shell/PTY 生命周期或当前不能安全自动化的 OS 行为才使用。运行命令和查看日志本身不是手动测试。
> - 测试按 Rust 单元/模块测试、维护脚本测试、集成资源测试和真实终端验收的最小充分层级放置。
> - 跨平台逻辑使用 `cfg` 与对应 CI/目标验证；不要用运行时条件跳过来伪装覆盖。

### 测试文件：`src/bus/orchestrator/tests.rs`

- Packed production bundle loads through the `production` selector with `ROOM_AGENT_CONTENT_INTERFACE_V1`, compatibility 1, a positive content version and a nonempty bundle digest.
- `system/system`、`agent/agent`、`index/index`、`skill/create-workflow`、`skill/execute-workflow`、`reference/workflow-template`、`reference/sop-review-plan`、`reference/sop-review-pr` and `reference/sop-execute-plan` are readable；an unregistered kind/name returns `UnknownEntry` without raw-path fallback。
- A test-only loader without a production bundle returns `ProductionUnavailable`；`test-agent-led` loading and reads are unchanged。
- Manifest schema, digest, flat-path, UTF-8 and cap violations remain enforced by the shared `build.rs` validation used for both selectors；no second runtime validator is added。

### 测试文件：`scripts/test_orchestrator_content_check.py`

- The checked-in production tree passes：required manifest entry names and flat files exist, the workflow template has every required section and exactly one Mermaid flowchart, and each built-in SOP has exactly one Mermaid flowchart。
- Temp-copy variants with a missing required entry, a missing template section, zero or two Mermaid flowcharts, or any removed prerequisite token in CamelCase or snake-case form each produce a diagnostic naming the file。
- The checker does not parse Mermaid control flow, classify wording, execute a model, or duplicate `build.rs` schema/digest/cap validation。

### 测试文件：`scripts/test_skill_goal_ownership_check.py`

- Each of six canonical skills routes through the shared consumer：`verified` is the orchestrated branch, local `NotInBusRoom` is the only standalone branch, and every other result blocks。
- Orchestrated coding-agent paths consume rather than rewrite Goal/Non-goals；plan/`.locked-goal` and `.locked-non-goals` exact match or fail closed；Orchestrator remains the room-level brief owner through `ProposeRoomBrief` and Human confirmation。
- Orchestrated `execute-plan` bypasses the standalone Python progression driver and performs only the bounded task/review/remediation/finalization operation selected by the room Orchestrator。
- No duplicate orchestrator-only skill appears under top-level/discovery skill roots; every discovery link still resolves to canonical source.
- Entry points remain under 250 lines; no severity/priority review labels or duplicate authority prose are introduced.
- Handoffs treat human/orchestrator/coding agents as first-class participants with explicit asymmetric capabilities and preserved provenance.

### 测试文件：`scripts/test_room_assignment_context.py`、`skills/pr/scripts/test_pr_format_check.py`、`skills/review-pr/scripts/test_review_round.py`

- Shared consumer returns `NotInBusRoom` without invoking the verifier only when the outer frame and all four Bus discovery variables are absent。A frame without complete discovery, discovery without a frame, or any partial discovery returns a blocker without invoking the verifier。
- Frame plus complete discovery invokes exactly `BUS_BINARY` with `--bus assignment verify --frame <frame>`；`verified` exposes the harness-validated payload for Goal/Non-goals precedence, while `invalid`, `absent`, malformed output, executable unavailable, timeout and nonzero exit block。It has no direct assignment-directory、record、pointer、token or authoritative Request reader and no second provenance validator。
- Create-plan copies trusted Goal/Non-goals verbatim；plan-bound skills exact-match them；planless PR context writes and reuses both `.locked-goal` and `.locked-non-goals`。Direct `review_round.py` tests prove matching pairs succeed and either lock missing/blank or plan mismatch fails closed；`test_pr_format_check.py` continues to own PR context production rather than standing in for review-pr consumption。

### 测试文件：`scripts/test_plan_review_finalization.py`

- An explicit Orchestrator-authorized separate-author operation changes only `review-plan-in-progress` to `review-plan-complete` when every required canonical lane is Ready for the exact current plan hash。
- Zero-finding first round and repaired later round succeed；missing/finding/stale/mixed-hash/duplicate-lane artifacts fail closed without modifying the plan。
- Reviewer remains read-only and no runtime/helper chooses the next workflow action after finalization。

### 单元测试

编写自动化测试用例来验证各个代码单元的功能正确性，确保代码在隔离环境中按预期工作。

**重要**

测试应覆盖以下路径：

- **成功路径**：正常操作流程
- **回退路径**：当主要方案不可用时的明确降级处理（降级语义必须可观察、可测试）
- **错误路径**：错误处理与不变量违反

Rust tests live beside their owners or in the repository's existing test modules. Python helper tests use `unittest` or `pytest` according to the neighboring suite; integration assets use the existing Bun harness. For PTY, process, timing, or terminal behavior, state whether the proof uses a deterministic backend/clock or a controlled real process.

#### 测试文件：`src/bus/orchestrator/tests.rs`

```rust
#[test]
fn room_orchestrator_core_content_production_bundle_is_selectable_and_readable() {
    let bundle = ContentLoader::test_bundle().load(ContentSelector::Production).unwrap();
    assert_eq!(bundle.interface, ROOM_AGENT_CONTENT_INTERFACE_V1);
    assert!(bundle.read("reference", "sop-execute-plan").is_ok());
    assert_eq!(bundle.read("reference", "missing"), Err(ContentError::UnknownEntry));
}
```

#### 测试文件：`scripts/test_orchestrator_content_check.py`

```python
class CheckedInContentContractTests(unittest.TestCase):
    def test_checked_in_production_tree_passes_objective_checks(self):
        self.assertEqual(validate_repository(REPO), ())

    def test_removed_capability_token_is_rejected(self):
        self.assert_rejected(with_removed_token_in("execute-workflow.md"))
```

#### 测试文件：`scripts/test_skill_goal_ownership_check.py`

```python
class LockedRoomBriefContractTests(unittest.TestCase):
    def test_orchestrated_and_standalone_paths_have_one_goal_owner(self):
        self.assertEqual(validate_goal_ownership(REPO), ())
```

### 全局 EXIT CHECK

按顺序运行：

1. `just lint`
2. `just test`
3. `just build`

> **重要**：
> - 前两步必需且顺序固定。`just build` 是可选的第三个槽位，仅在计划行为需要构建证明时保留。
> - 每个槽位只接受与当前 `justfile` 一致的完整仓库命令，不接受 task filter；计划特有的命令属于 `[TASK_LOCAL]` 闸门。
> - 需要 Windows 证明时，按影响范围额外运行 `just windows-lint` 或 CI 的 Windows gate，并如实记录当前宿主无法完成的真实终端交互。
> - 无法由 agent 完整运行的设备 / 视觉验收只进入「手动测试」。

本计划保留 `just build`，因为 `build.rs` 会把 production bundle 校验并打包进 release binary。新增 Python contract suites 进入 `maintenance-test`，Rust content tests 属于全量 `cargo nextest`，因此 final `just test` 在任意 repair 后重放同一 objective evidence；不存在 acceptance driver 或 replay target。Fixtures 不执行模型、不证明 semantic orchestration quality，所有自动 gate 的 live provider calls 为零。

### 手动测试（仅在无法自动化时使用）

提供详细的步骤说明，指导测试人员通过实际运行应用程序来验证功能的正确性和用户体验。

**重要**

- **大多数计划不需要此部分**。如果所有测试都可以通过自动化测试覆盖，应删除此部分。
- 仅在确实需要人眼判断、主观评估或无法建立的 GUI 交互 Session 时才保留此部分。
- 提供**逐步测试说明**，使用清晰、有序的要点。
- 对于每个步骤，说明**测试者应该看到什么**或**正确的结果是什么**。
- **步骤内描述一律用无序列表 `-`，禁止用 `1. 2. 3. 4.` 有序编号**：步骤本身用 `- [ ] 步骤 N` 标签编号即可，步骤内的操作与预期结果用 bullet（`-`），增删条目无需手动重排。
- **注意**：开发者应在完成每个测试步骤时勾选对应的复选框。
- 人工验证不能冒充自动测试；最终回复必须分别列出已验证证据和仍需人工检查的事项。

#### 场景 1：开发者授权的 live DeepSeek production-content 端到端检查

- [ ] **步骤 1**：开发者使用前置计划已交付的 Orchestrator settings 与 credential 入口提供自己的 DeepSeek key，并选择 `production` content。
  - **预期结果**：Bus 正常启动 Orchestrator；这是本计划唯一的 live provider 使用，且只由开发者明确授权并手动发起。
- [ ] **步骤 2**：在新 room 中向 Orchestrator 描述一个 bounded 需求。
  - **预期结果**：Orchestrator 只提出 Room Brief proposal（Goal/Non-goals）；Human exact confirmation 前不读取 content、不写 draft，也不 dispatch coding agent。
- [ ] **步骤 3**：确认 Room Brief 后继续协作该 workflow。
  - **预期结果**：Orchestrator 按需以 `ReadContent` 读取 skills、template 或 built-in SOP，以 `SendMessage` 委派技术工作并以 room message 请求 Human 决策；记录观察结果与被检查的 tree，不把该人工检查当作自动 gate 证据。

> **⚠️ 不可修改**：以下规则部分必须包含在每个计划文档中，AI 和开发者不得修改此部分。

<!-- 计划执行规则：开始 - 此部分不可修改 -->
## 计划执行规则

- AI 只以计划状态判断评审就绪：首次执行必须为 `review-plan-complete`，恢复执行允许为 `plan-execution-in-progress`。
- `execute-plan` 不再重复检查计划完整程度或逐文件审查状态；文件契约、任务 owner、依赖图与验收闸门仍必须通过结构和安全校验。
- **最小变更原则**：
  - 仅修改任务直接要求的代码
  - 除非明确要求，否则不得重写、重新排序或重构不相关的文件或模块
  - 除非必要，否则不得修改空白字符（不删除空行、不添加空行、不更改缩进或格式）
  - 保留所有现有的命名、风格、模式和架构
  - 不确定是否需要额外的自定义逻辑、抽象或新结构时，应停止并请求人工确认，而不是发明新机制
- **注释质量原则**：
  - 不要生成重复代码内容的注释
  - 不要描述函数名、参数名、返回类型或基本逻辑（循环、空值检查、简单条件判断）
  - 仅在解释**为什么**时添加注释，而非解释**是什么**
  - 允许的注释内容：非显而易见的逻辑或行为、关键假设或约束、平台特定问题、副作用或生命周期交互、代码中不明显的重要推理
  - 宁愿**不添加注释**，也不要添加无意义或冗余的注释
  - Public-repository comments must use concise English
- **禁止 TODO 原则**：
  - 不得编写 TODO、FIXME、XXX 或占位符注释
  - 不得留下存根实现、空代码块、静默降级或伪造成功
  - 生成的每段代码必须完整、具体且可在上下文中运行
  - 如果无法完全实现某项功能，应停止并请求澄清，而不是猜测或留下占位符
- **任务范围原则**：
  - **一个计划 = 一次性执行 = 一个 PR，与大小无关**：任务图只用于安全并行和硬依赖调度，不是人工断点；编排方不得将非法图或资源限制不透明地静默串行化。
  - 执行开始前记录 `DIRTY_BASELINE`；与任务 owner 重叠的预存脏路径必须 STOP，不重叠的 baseline 必须保持逐字不变。
  - 执行期任务只能写自己的 owner；发现 owner gap 必须停止并修计划，不得越界。
  - 每个任务必须发布与当前 generation、文件内容和 gate 日志绑定的 evidence，再经过 `/execute-plan` 内部 task review。
  - 全部任务 Ready 后，在同一 candidate tree 上运行完整 EXIT CHECK；任何修复改树都从 final lint 重建证据。
  - 最终工作树必须包含任务 checkbox、deviation report 和 `plan-execution-complete` 状态，再计算 tree hash。
  - 全部 task Ready 后，`/execute-plan` 在 final gate 前机械写入 `plan-execution-complete`，使状态本身进入被验证 tree；任一 final gate 失败或 tree 漂移必须恢复 in-progress，修复后重新写入 complete 并从 final lint 重建证据。
  - agent-driven e2e 必须在该精确 tree 上运行；人工 e2e 只记录待验证 tree 与清单，不冒充通过。
  - 任务内不得出现 `(需要手动操作)`；人工验证只存在于「手动测试」section。人工结果若是后续实施的硬前置，必须拆成两份计划。
  - task acceptance 只是局部安全门；人工验证后仍必须运行完整 `/review-pr`。
  - XXL/XXXL 也使用同一粗粒度任务图；任务数量不按计划大小机械扩张。
  - 无需考虑渐进式迁移策略，应直接完整实现所需功能。
- **Git 落盘原则**（以 [`commit-and-push`](../../skills/commit-and-push/SKILL.md) 为 SOT）：
  - 从进入 `plan-execution-in-progress` 到全部自动 gate 完成，所有 actor 必须零 Git：不得 `git add`、`commit`、`push`、`stash`、`rebase` 或切换 ref。
  - 自动 gate 全绿后只允许以 `commit-and-push` 作为最后一步落盘，之后不得再改树。
  - 默认不在共享分支落盘；开发者明确要求直接在 `master` commit-and-push 时，该要求授权普通提交和显式 `git push origin master:master`，但不授权 force-push。
  - feature 分支落盘后交回开发者人工验证，再由 `pr` 创建或更新面向 `master` 的 PR，并由 `review-pr` 完成最终审查；`execute-plan` 自身不 rebase、不创建 PR。
<!-- 计划执行规则：结束 -->
