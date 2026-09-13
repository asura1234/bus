# Skill 多层架构

本文定义 Bus workflow skill 的信息分层、单一事实源和中间产物协议。目标是让 agent
易于执行、让机械约束可验证，并避免同一规则散落在多份自然语言文档中持续漂移。

## 核心原则

一个完整 workflow skill 由四类职责组成：

| 层级   | 载体                    | 负责                                                               | 不负责                               |
| ------ | ----------------------- | ------------------------------------------------------------------ | ------------------------------------ |
| 执行层 | `SKILL.md`              | 可执行伪代码、命令顺序、分支、循环、STOP 条件、其他 SOT 的读取时机 | 长篇原理、完整格式模板、手工解析逻辑 |
| 原则层 | `guide.md` 或共享 guide | 判断原则、边界、优先级、风险与例外                                 | 字段顺序、固定标题、机械校验实现     |
| 机械层 | 确定性脚本              | 解析、标准化、校验、状态计算、fail-closed gate、确定性渲染         | 需要语义判断的评审结论、架构取舍     |
| 格式层 | `*-format.md`           | 中间产物和最终产物的严格结构、字段、枚举、顺序、空值表达           | 工作流顺序、判断原则、脚本实现细节   |

同一规则只允许有一个权威层。其他层只能引用，不得复制一份近似表述。

## 1. `SKILL.md`：可执行伪代码

`SKILL.md` 是 agent 的执行入口，应当像可运行的控制流：

- 逐步列出读取、调用、分支、循环、等待和终止条件。
- 明确何时必须读取 guide 与 format。
- 机械步骤直接调用脚本，不要求 agent 临场重写解析逻辑。
- 只保留执行所必需的短指令；“为什么”下沉到 guide。
- 不嵌入完整报告模板，只引用对应 `*-format.md`。
- 不手工改写脚本输出；需要固定聊天输出时，逐字返回 renderer 产物。

`SKILL.md` 必须保持简洁。若引入硬性行数上限，应同时提供确定性检查并接入
`just maintenance-test`，不得只在文档中声明一个无法验证的门禁。入口超长的典型原因不是流程
变复杂，而是 guide 原则、产物模板或脚本伪实现回流到了入口。

## 2. `guide.md`：指导原则

guide 负责需要 agent 理解和判断的规则：

- 目标、非目标、职责边界和优先级。
- 如何处理不确定性、冲突、异常和合理偏差。
- 何时升级给主 agent 或开发者。
- 为什么某些 fail-open 行为不可接受。
- 哪些检查属于机械证明，哪些仍需要语义判断。

guide 不应复制 `SKILL.md` 的完整步骤，也不应维护 artifact 的标题、字段顺序或固定模板。
执行入口必须在相关判断发生前明确要求完整读取对应 guide，不能假设 agent 会自行发现。

## 3. 确定性脚本：机械检查与 gate

低自由度、容易漂移或需要重复执行的工作必须下沉到脚本：

- fence-aware / section-aware 解析。
- 路径、任务、lane、hash 和状态身份计算。
- schema 与枚举校验。
- fail-closed 前置门禁。
- 确定性裁剪、汇总和渲染。
- stable stdout、JSON 或 exit code 契约。

脚本必须：

- 对畸形、缺失、重复、顺序错误和未知枚举返回非零。
- 不通过猜测补全缺失字段。
- 让同一输入产生同一输出。
- 为正常路径、边界路径和 fail-closed 路径提供自动化测试。
- 与 format SOT 使用同一字段和枚举，不维护第二套隐式 schema。

脚本可以证明结构和状态转换，不能证明模型真的完成了语义审查。后者依靠 reviewer
复核、跨模型 dogfooding 和最终人工判断。

## 4. `*-format.md`：严格产物格式

凡是需要跨 agent、跨轮次或跨 skill 消费的 artifact，都必须有独立格式 SOT。文件名使用
具体领域前缀，例如：

- `review-format.md`
- `task-agent-report-format.md`
- `execute-plan-action-format.md`

格式文档必须钉死：

- 标题和 section 的固定顺序。
- 必填、可选和禁止字段。
- 合法枚举和大小写。
- 空集合、无问题和不适用的唯一表达。
- 路径、编号、hash 与时间等字段的规范化形式。
- 不同状态下必须出现或禁止出现的内容。
- 完整正例，以及必要的畸形反例说明。

仅单个 skill 使用的格式放在该 skill 的 `references/`；多个 skill 共用的格式放在共同的
权威位置，并由所有消费者直接引用。禁止在 `SKILL.md`、guide 和脚本注释中再复制完整模板。

## 中间产物生命周期

标准 artifact 流程如下：

```text
agent 按 *-format.md 写 artifact
  → parser / gate fail-closed 校验
  → renderer 生成精简 completion envelope
  → producer agent 逐字返回 envelope
  → consumer agent 按 SKILL.md 状态机处理
```

完整证据留在 artifact 文件；agent 间消息只传状态、摘要和 artifact 路径。这样既减少主
agent context 污染，也避免由模型手工摘要造成字段丢失。

若 artifact 未通过机械校验：

- producer 不得报告完成。
- consumer 不得从自然语言猜测状态。
- producer 修正 artifact 后重新运行 gate。

## Agent 间通信

Agent 间通信必须区分两类信息：

- **控制消息**：短、结构化，用于唤醒和驱动状态机。
- **证据 artifact**：完整、持久化，供后续验收、resume 和审计。

控制消息至少包含状态、任务身份和 artifact 路径。状态必须是 format SOT 定义的闭集，
不得用“基本完成”“应该可以”等自由文本替代。详细 touched files、gate 输出和 concerns
写入 artifact，不粘贴进控制消息。

Subagent 遇到需要主 agent 行动的状态时必须立即回报，不等到假想的“最终完成”：

- 缺少上下文。
- owner gap。
- 无法继续的 blocker。
- 已完成但存在正确性疑虑。

主 agent 只等待已知仍在运行的 agent；有本地编排或验收工作时先做这些工作。等待应使用
宿主允许的最长安全窗口，并在任一 agent 回报后立即处理，不做连续短间隔 busy-poll，也不
反复询问“完成了吗”。

## 目录与引用

推荐结构：

```text
skills/<skill>/
├── SKILL.md
├── guide.md
├── references/
│   └── <artifact>-format.md
└── scripts/
    └── <mechanical-task>.py

.agents/skills/<skill> -> ../../skills/<skill>
.claude/skills/<skill> -> ../../skills/<skill>
```

`skills/` 是唯一可编辑的权威副本；`.agents/skills` 与 `.claude/skills` 只是发现链接。
`SKILL.md` 必须直接链接本次执行需要的 guide 和 format；不要形成 guide → reference →
reference 的深层链路。脚本通过命令调用即可，除非正在修改脚本，否则 agent 不需要先读取其源码。

## 变更检查清单

修改或新增 workflow skill 时：

1. 判断新规则属于执行、原则、机械还是格式层。
2. 只修改该规则的权威 SOT，其他层只补引用或调用。
3. 新增或修改 artifact 时先更新 `*-format.md`，再更新 parser / renderer。
4. 为机械契约补正常与 fail-closed 测试。
5. 确认 `SKILL.md` 保持简洁，硬性约束均有自动检查。
6. 运行适用的 `just lint`、`just test-one <filter>`、`just maintenance-test` 与 skill
   symlink 检查。
7. 用真实任务 dogfood；只把重复失败或高风险 fail-open 固化为新控制面。

## 禁止模式

- 把指导原则、格式模板和脚本伪实现全部堆进 `SKILL.md`。
- 同一字段或枚举在多份文档各写一套。
- format 声称严格，parser 却接受缺段、乱序或未知状态。
- parser 产生一种结构，renderer 再用另一套规则解释。
- 让 agent 手工摘要或改写本可确定性生成的输出。
- 仅凭 artifact 文件存在就宣称完成，未运行机械 gate。
- 为一次低影响偏好反馈增加新的状态机、指标或第二事实源。
