# Feishu notifications for developer attention

**状态**：plan-execution-complete
**作者**：dylanliu8949
**创建日期**：2026-10-18
**基于提交**：a083dee0
**分支**：feat/feishu-notify
**前置任务（如适用）**：无
**后续任务（如适用）**：无

<!-- 规则优先级：开始 - 此部分不可修改 -->
## 规则优先级

1. 开发者在对话中的最新明确要求。
2. 本计划的目标、非目标与已归档决策。
3. 本模板、计划指南与代码现状。

事实必须区分为：当前源码已验证、开发者明确决定、待验证假设、延期工作。POC 的 ceiling 假设不得写成生产承诺。

- 计划生成规则和计划执行规则优先于模型的隐式行为
- 当任务指令与计划生成规则或计划执行规则冲突时，必须遵循这些规则
- 如果由于任务约束无法遵循计划生成规则或计划执行规则中的某条规则，应暂停并请求澄清，而不是猜测
- 如需覆盖这些规则，应更新计划模板文档，而不是在单个计划文档中覆盖
<!-- 规则优先级：结束 -->

## 目标

Bus delivers developer-action notifications to the authenticated lark-cli user themself so agents can run unattended across office machines, with runtime controls and clear MASTER badges.

> **重要**：
> - **目标与非目标共同标示本计划的意图（intent）与范围（scope）**：目标声明**要做什么 / 交付什么**（**强制**——写任何计划正文前必须先清晰设定），非目标声明**刻意不做什么**（**可选**——未声明即视为无、AI 不问不猜，但**一旦声明即被强制执行**）；二者一起把计划的意图与边界钉死，是后续 create / review / execute 全程锁定的地基。
> - 计划必须**单一目标**。判断标准是**目标是否内聚**，不是任务数量——一个 XXXL 计划可以有多个粗粒度任务，只要它们共同服务同一个目标，它就仍是单一目标。
> - 类比：「建一座动物园」是单一目标，即使内部有建狮笼、建鸡舍、修围栏等多个任务，它们仍共同拼成一个成果。若顶层目标是多件互不相关的事，则应拆成多份单一目的计划。
> - 目标陈述聚焦**做什么 / 交付什么**（结果），不描述**怎么做**（实现细节留给后续章节）。
> - 计划一律 one-shot 执行、执行后再做 e2e 验证、一个计划一个 PR，与大小无关；不要把「单一目标 + 大」误当成「多目的」而拆散。
> - **目标在计划撰写阶段定稿后即锁定**：只有开发者可修改。plan review / 任何 agent **不得推翻、扩张、缩小或重新定义**它，只能检查计划正文是否服务于该目标（详见 [`docs/guides/plan-review-guide.md`](../guides/plan-review-guide.md)「工作流结构是既定常量」）。

## 非目标

- Replies from Feishu to Bus.
- Worker-authored attention tagging; workers report through their orchestrator.
- Any configurable recipient, chat or group destination.
- Driving the live Bus or sending real Feishu messages without the task owner's explicit prior approval.

> **重要**：
> - **可选，但设了就强制**：开发者未显式声明即写「无」——AI **不问、不猜、不外推**（宁可留「无」，不要编）；**一旦声明了任一条非目标，它即被强制执行**（往其方向推进的评审意见一律驳回，见下）。这是与目标的关键差别：目标**强制必设**，非目标**可留空、但设了不可犯**。
> - 与目标一样**锁定**：定稿后只有开发者可改；plan review / 任何 agent 不得新增、扩张或重新定义非目标。
> - 往非目标方向推进的评审意见（如「顺便也做 X」而 X 正是某条非目标）= 扩范围熵，一律驳回（见 [`docs/guides/plan-review-guide.md`](../guides/plan-review-guide.md) 与 [`docs/guides/code-review-guide.md`](../guides/code-review-guide.md)）。

## 当前状态分析

Source verified:
- `src/messaging/coordinator/worker.rs` owns the 500 ms coordinator tick, durable save boundary, callback ingestion and settings path. Global preferences live in `src/messaging/prefs/settings.rs`; commands flow through `BusCommand`, control method specs and the existing settings UI.
- `model/rooms.rs::post_to_developer` records the requested room. Explicit orchestrator `send --room master --to human` posts and replies to developer MASTER requests reach MASTER. `coordinator/reports.rs` routes worker/background-started finals to the work room. Tag detection covers actual MASTER posts and replies; guidance must explicitly post to MASTER when the turn final would land elsewhere.
- `coordinator/dialogs.rs` already stabilizes and deduplicates blocked episodes; `model/requests.rs::stall_reason` already distinguishes actual timeouts from long Working or Blocked turns. Reuse these semantics, including queued expiry, without imposing a new timeout on healthy work.
- `coordinator/usage.rs` owns provider quota observations. Today only Claude display applies the 15-minute age/reset filter; Codex display does not, and its parser drops event timestamps while provider_turns records read time. Preserve the provider observation timestamp in the existing Codex parser, and put the shared alert freshness predicate in Usage: known timestamp no more than 15 minutes old, not future, with a future reset. Missing/stale/expired evidence is unknown for alerting; only fresh reported windows at >=100 percent qualify. Cursor has no existing usage source.
- Terminal state already records a recent explicit agent process exit. Native AgentList omits exited agents and does not export that marker. Add a minimal pane snapshot marker derived from the existing terminal state, excluding Installing/Relaunching self-update phases at the native owner; a missing AgentList entry or API error alone is never proof of exit. Query a bound pane only when an agent disappears; confirm terminal identity and explicit marker, skip intentional deletion and self-update/relaunch.
- The current notice loader drops every Author::Bus notice. Retain only the new tagged mechanical notices across save/load, preserving legacy untagged cleanup.
CLI evidence verified locally (no messages sent): `lark-cli --help`, `lark-cli auth --help`, `lark-cli auth status --help`, `lark-cli im +messages-send --help`; real `auth status --json --verify` returns top-level `identities.user.openId`, `available`, `verified`, `status` and `scope`, rather than the ordinary API ok/data envelope. Verified `needs_refresh` is usable when server verification succeeded. Send help defines `--as user`, `--user-id`, `--text -` and `--idempotency-key`.
Developer decisions: default off; self only; fixed [Bus] prefix; no live sends during implementation. Remaining execution details use adjacent Rust patterns. No claim of Windows runtime validation on this macOS host.

> **重要**：
> - 仅包含与任务直接相关的内容
> - 说明当前实现的关键点，并区分「源码已验证」与「文档所述但未复核」
> - 列出可能影响实现的技术约束（平台边界、进程边界、依赖方向、Profile 隔离等）
> - 需要图示时使用 mermaid，描述相关组件如何组合在一起；不引入未配置的绘图工具

## 参考资料

- `docs/templates/plan-template.md`, `docs/bus-architecture.md`, `docs/guides/architecture-principles.md`.
- `src/messaging/coordinator/{worker,settings,dialogs,poll,usage,reports,callbacks,expiry,storage}.rs`, `src/messaging/coordinator/control/{mod,inspect,messages}.rs`.
- `src/messaging/model/{types,rooms,requests,compactions}.rs`, `src/messaging/prefs/settings.rs`, adjacent coordinator/model tests.
- `src/client/rooms/{input,render,history}/`, `src/cli/control/parse.rs`, `src/cli/help.rs`, `orchestration/{README,prompt,rules,how-to-bus-cli}.md`.
- `src/terminal/state/{mod,detection}.rs`, `src/server/api/events.rs`, `src/protocol/api/schema/panes.rs`, schema golden generator; `src/client/compositor/tests/compositor_test.rs` constructor.
- `src/agents/providers/codex/usage.rs`, `src/agents/providers/codex/tests/usage_test.rs`, `src/messaging/coordinator/provider_turns.rs` and usage tests.
- `src/devtools/tui/fake_agents.rs`, `src/devtools/tui/session.rs`, `tools/acceptance/harness_test.py`, `skills/gate-and-fix/scripts/bus_quality.py`, `pytest.ini`.
- Installed `/opt/homebrew/lib/node_modules/@larksuite/cli/scripts/run.js` (Node wrapper calls native grandchild), `src/agents/providers/claude_code/statusline.rs` private process-group timeout pattern.
- `/Users/dylanliu/.agents/skills/lark-im/SKILL.md`, its `references/lark-im-messages-send.md`; lark-shared identity and output-contract references; the locally executed help commands above.
- `justfile`, `docs/tui-driver.md`, `skills/e2e-test/`, `skills/gate-and-fix/`, `workflows/auto-merge-pr.md`.
- Common lane rules: `/Users/dylanliu/.local/share/bus/sessions/585d051bcf3a8551/workflows/bus-lane-brief-2.md`.

> **重要**：
> - 使用官方库 / 框架 / SDK 文档，并在本节记录实际读取的链接
> - 如果任务涉及现有模块，必须读取相关源码、调用点与测试
> - 如果任务跨模块或触及仓库级契约，必须包含对应的根文档、`docs/` 文档或 `skills/skill-architecture.md`
> - 如果任务涉及第三方 SDK，应包含以下链接：
>   - SDK 集成文档
>   - 如何启用 XX 功能的文档
> - 创建计划时由现有模块、调用点与测试确定职责、依赖方向、公开 API 与验收入口
> - 新增的模块、文件、类型、函数命名应遵循相邻代码中体现的命名约定
> - 如果计划包含测试，应参考相邻 Rust `#[cfg(test)]`、模块测试、`scripts/test_*.py` 或集成资源测试的既有模式
> - 如果计划涉及日志输出或运行时不变量，必须读取实际拥有该行为的 Rust 模块和现有测试
> - 如果计划涉及跨平台行为，必须说明 Unix/macOS 与 Windows 各自的可用验证入口
> - 如果此任务基于另一个任务，或未来任务依赖此任务，应包含相关任务计划文档的路径
> - 如果找不到或未提供所有相关文档，AI 应停止计划生成并通知开发者

## 需要决策的事项

**当前计划完整程度**：100%

无待确认事项。The task owner supplied the complete feature and execution contract, including both reviewer models, three-round caps, scratch-only E2Es and preauthorized squash merge.

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

1. Developer: only an orchestrator's MASTER message starting with `[needs_developer_attention]` signals a decision/action that blocks work. Recognize the exact leading token (after leading whitespace), strip it only in display, render a visible badge, and preserve raw text in history. Untagged progress, human text, worker text and internal agent messages do not send.
2. Developer steering: recipient is NOT configurable. Resolve `identities.user.openId` at enable time, store it only in a private authenticated-self handle, expose no recipient argument/config/CLI/control field. Send only `--as user --user-id <resolved-self>`. Reverify auth before each send and disable with an error if identity changes or is unavailable; no fallback to bot/chat/group. Every outgoing body begins `[Bus]` and includes hostname, Bus session, originating room, agent identity and complete message text. Tests assert self-only argv and reject all alternate-recipient inputs.
3. Runtime control: `Feishu notifications` checkbox plus `bus settings feishu-notifications --on|--off`, available without --dev. Persist only a boolean defaulting false using the existing global settings file. All auth validation runs off the coordinator thread, with explicit pending/effective/error runtime status. The toggling session persists true only after successful validation; failed direct enable leaves it off. Startup/other-session observers validate independently; their failures set only local effective-off/error and never write false to a shared true preference. Observed file changes emit a UI update. Off cancels pending validation/delivery immediately; stale completions cannot enable it. CLI waits for pending validation and returns the refusal reason, while the UI stays responsive. Required user scopes are exactly `im:message.send_as_user` and `im:message`, from lark-im. Re-enable resolves self anew.
4. Transport: separate module under messaging owns lark subprocess execution, private self handle, bounded background delivery and timeout/error handling. No shell interpolation. Use stdin for message text, a 10 second deadline covering process IO/completion, bounded output, and terminate/reap the private process tree on timeout/cancellation (Unix private process group as in statusline; Windows corresponding tree termination). The installed Node wrapper has a native grandchild, so a direct-child kill is insufficient. Test a wrapper/grandchild holding pipes. No automatic send retries after ambiguous outcomes; verify exactly one send invocation on failure/timeout. Validation and delivery must not block the coordinator. Do not persist a user ID or credentials, log raw auth output, or expose it in errors.
5. Flood control: dedupe identical origin+text for 10 minutes and immutable source IDs; send at most six notifications per rolling minute. Retain distinct messages in a bounded pending queue of 128; if full, leave the source unconsumed in existing durable MASTER history and retry enqueue when capacity returns, with visible backlog status. Do not drop distinct tagged messages or allocate an unbounded second queue. Disable invalidates the queue and prevents new sends; an already launched subprocess can finish. Errors remain visible and never recursively generate Feishu notifications. Persist per-source delivery disposition in session state: sources ignored at the first-enable baseline, queued/unattempted sources, and attempts recorded durably before a send invocation. Ordinary restart with the preference on resumes unattempted backlog after fresh local auth; it never re-baselines that backlog. Completed, failed or ambiguous attempts are never replayed; an interrupted attempt is surfaced as outcome unknown. Persist recent attempt times and content signatures so restart preserves the six-per-minute limit and 10-minute content dedupe. Explicit off intentionally cancels/skips pending sources, and a later explicit re-enable establishes a new baseline; pre-enable/off-period history does not notify. This is at-most-one automatic attempt per source, not a claim of exactly-once platform delivery.
6. Mechanical messages: Bus-authored tagged MASTER notices, with origin/agent in body; stable blocked orchestrator episodes, explicit provider exit evidence, fresh exhausted provider windows, and existing stalled request reasons. Do not tag worker dialog notices, routine resolved episodes, intentionally deleted/restarted processes, API outages, stale/expired quotas or healthy Working turns. Reuse the existing durable dialog_notice episode: replace the one blocked MASTER-orchestrator notice with the tagged Bus notice, never add a second one; preserve worker routing and untagged resolved notices. Other mechanical episode keys persist in BusState atomically with notices and re-arm only on positive recovery evidence, not temporary unavailability during restore. Reopen tests must prove no new notice/send for the same persistent condition. Dedupe controls repeated message text.
7. Guidance changes: rules say a tag works only on a MASTER reply or `bus send --room master --as SELF --to human --text "[needs_developer_attention] ..."`; when a worker/background started the turn, explicitly post that message to MASTER because the final goes to the work room. The prompt report section gets exactly one line: "When you need the developer to act or decide and work waits on them, start your report with [needs_developer_attention] and follow the rules to deliver it in MASTER." Routine progress stays untagged. The task's end-to-end author workflow overrides generic template handoff pauses: review plan with Claude and Codex, TDD, E2E #1, full locked gate, draft PR, Claude+Codex code review, E2E #2 at final PR head, authorized squash merge and worktree close. Cap each review phase at three rounds; report sharp disagreement. Never send real Feishu without asking the task owner first. An optional real receipt/phone-alert check requires separate approval of its exact self-only [Bus] payload and identity; it is not an added prerequisite to the authorized stand-in/gate/review/merge workflow. Real platform receipt/alert remains explicitly unproven unless that approved check runs.



## 大小

**大小**：XL



<!-- 计划生成规则：开始 - 此部分不可修改 -->
## 计划生成规则

- **作者**字段必须填写 GitHub 用户名（通过 `gh api user -q .login` 获取），不得使用 "claude_code"、"AI" 等非人类标识符。此字段用于追踪计划质量归属。
- **分支**字段必须是当前具名分支。默认使用 feature 分支；只有开发者明确要求时才允许直接在 `master` 落盘。
- 一份计划只有一个目标；大小不等于多目的。
- AI 生成的计划文档必须包含本模板中的所有部分；标记为「（如适用）」的部分是可选的，开发者可以选择主动删除。
- 所有 `> **重要**：` 块必须从模板中复制，不得修改或省略。
- 开放决策存在时，保持 `create-plan-in-progress`，不生成文件契约、任务图或测试计划。
- **当前计划完整程度**初始应留空，AI 不得自动填写百分比；随着开发者做出决策并解决「需要决策的事项」，AI 应更新此百分比。
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

One integrated task owns this feature's coupled source/test/docs changes. No module migration or deletion.

| Files / stable cluster | Responsibility / conceptual contract |
| --- | --- |
| `src/messaging/attention.rs` (new), `src/messaging/mod.rs` | Single exact tag parser shared by model, coordinator and UI: `attention_text(text: &str) -> Option<&str>`. |
| `src/messaging/feishu/` (new, including tests) | Authenticated-self-only transport and bounded scheduler. `enable() -> Result<SelfIdentity, String>`; `enqueue(origin, text, source)` accepts no recipient; `disable()` invalidates pending work. Process runner receives executable/argv/stdin, bounded deadline/output, no arbitrary destination. |
| `src/messaging/coordinator/attention.rs` (new), `mod.rs`, `worker.rs`, `commands.rs`, `settings.rs`, `storage.rs`, `expiry.rs`, `poll.rs`, `dialogs.rs`, `provider_turns.rs`, `usage.rs` | Coordinator owns enable/disable, scans newly committed MASTER notices and completed replies once, synthesizes mechanical notices using existing facts, observes global bool changes, exposes runtime errors. Concept: `refresh_attention(now)` → detect episodes → save notices → enqueue new durable tagged messages. Never send an unsaved change or replay initial history. |
| `src/messaging/coordinator/control/mod.rs`, `control/inspect.rs` | `settings.feishu_notifications` accepts only `on: bool`; strict unknown-field rejection, Agent tier. Settings JSON projects pending/effective/error from the coordinator; it never exposes a recipient. The CLI starts validation then polls that read-only status to return success or a concrete refusal without blocking the coordinator. |
| `src/messaging/model/attention.rs` (new), `rooms.rs`, `state.rs`, `types.rs` | Narrow `post_developer_attention(episode, origin, text, now)` writes one tagged Author::Bus MASTER notice plus its durable episode latch atomically. A serde-defaulted optional `Prompt.attention_origin` captures room/agent IDs and display names for mechanical metadata even after deletion; `BusState.attention_episodes` persists mechanical latches with explicit positive-recovery clearing; serde-defaulted `BusState.attention_delivery` owns source dispositions plus recent attempt/signature data. Save an attempt before invoking the background sender; on restart leave interrupted attempts unknown and never retry them, while replaying only unattempted backlog. Source state is pruned with retained history, without a second durable copy of message bodies. Loader preserves only new tagged Bus notices; raw tags remain durable. Existing prompt constructors use None, and legacy untagged Bus cleanup remains. |
| `src/messaging/prefs/settings.rs` | `feishu_notifications: bool` with serde/default false; no destination fields. |
| `src/client/rooms/render/{mod,settings,hits}.rs`, `input/{mod,forms,settings}.rs`, `ui.rs` | Checkbox, action/hits/keyboard selection, field scrolling. Coordinator result owns effective on/off; refusal shown via existing visible error. |
| `src/client/rooms/history/exchange.rs` | Tagged eligible MASTER notice/reply gets a header badge and body without raw prefix. Concept: `display_body = attention_text(raw).unwrap_or(raw)`, gated by actual author and MASTER room. |
| `src/cli/control/parse.rs`, `src/cli/help.rs` | Public grammar/help for runtime toggle, with bounded polling of pending validation and a final effective-success/refusal response; no recipient flags accepted. |
| `src/terminal/state/mod.rs`, `src/server/api/events.rs`, `src/protocol/api/schema/panes.rs`, `src/protocol/api/schema/bus-api.schema.json` | Minimal existing-exit projection: `agent_process_exited: bool` (serde default, omitted false) derived from terminal's explicit exit latch and native self-update phase (false during Installing/Relaunching). Match pane+terminal in consumer; unavailable alone never counts. Regenerate schema with existing golden harness. |
| `src/messaging/coordinator/tests/attention_test.rs` (new), adjacent model/prefs/CLI/UI/native tests; `src/client/compositor/tests/compositor_test.rs` and `src/server/api/socket/tests/accept_test.rs` constructor fallout | RED/GREEN coverage of forwarding, mechanical episodes, UI badge/toggle, parser/strict recipient rejection, source durability and snapshot exit marker. Existing fixture constructors receive false marker only where compilation requires it. |
| `orchestration/rules.md`, `prompt.md`, `how-to-bus-cli.md`, `README.md`, `docs/bus-architecture.md` | Criteria + exact report-line instruction, enable command, prerequisites, dedupe/rate/error behavior, architecture SOT. |
| `tools/acceptance/feishu_notify_test.py` (new), `tests/e2e-tui/feishu-notify.md` (new), `src/devtools/tui/fake_agents.rs`, adjacent fake-agent tests, `docs/tui-driver.md` | Repeatable argparse-main scratch runner with no pytest-collectable tests, and QA run contract. Add a fake-provider raw-reply directive so a genuine final can start with the attention tag; document/test it. Build via `CARGO_TARGET_DIR=<worktree>/target ./run tui list`, then pass `<worktree>/target/tui-driver/debug/bus` explicitly to the runner. Scratch launch uses a closed PATH containing only the recording fake bin and system `/usr/bin:/bin:/usr/sbin:/sbin`, excluding Homebrew/node/real lark locations. Assert executable resolution to the stand-in before enable; missing-binary case removes the stand-in with no real fallback. Record argv/stdin; assert every send is user identity to the one fake self and starts [Bus]. Never real providers/Feishu writes. |
| `src/agents/providers/codex/usage.rs`, `src/agents/providers/codex/tests/usage_test.rs`, `Cargo.toml`, `Cargo.lock` | Existing parser returns original observation timestamp with quota windows; no second rollout parser. Enable the existing time crate parsing feature if needed. Missing timestamp stays unknown for alerts; preserve bounded rollout reading. |
| `plans/2026-10-18-feishu-notify.md` | This plan and execution checkboxes/evidence/deviations. |

Conceptual native marker:
```rust
pub struct PaneInfo { /* existing fields */ pub agent_process_exited: bool }
```
Conceptual coordinator control:
```rust
BusCommand::SetFeishuNotifications(on) => self.set_feishu_notifications(on, events)
```
Conceptual source policy:
```rust
// Only durable MASTER notices/replies authored by an orchestrator or Bus qualify.
if eligible_master_message && attention_text(raw).is_some() { enqueue_self_notification(origin, raw); }
```
Any changed behavior in the listed clusters follows these signatures/control-flow contracts; mechanical module declarations, imports, constructor false defaults and docs require no separate algorithms.

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
> - 改变 skill 架构或公开工作流契约时，`skills/skill-architecture.md` 与受影响的共享 guide 必须进入同一 owner 与 gate。

## 错误跟踪（如适用）

Enable errors return through the existing command/control error channel and leave the checkbox off. Notification subprocess/queue/auth-change errors are visible in the Bus snapshot/settings and structured tracing (`bus.feishu.failed`, stage and origin), with no raw auth JSON/credentials. Persistence failure cannot produce an outward send. Failed sends are not automatically retried; subsequent source events can still be evaluated subject to quota, and account/auth change requires re-enable. Existing control error envelope is retained; no new network protocol error taxonomy.

> **重要**：
> - 保持 Bus 当前日志与 stderr 边界；先从实际拥有该错误的 Rust 模块和直接消费者确认现有契约，不引入第二套日志 sink。
> - 级别按**结果语义**判断，不由是否进入 `catch` 或返回值真假机械决定：`debug` 开发期内部细节、`info` 正常关键轨迹、`warn` 异常但已按明确契约恢复或降级、`error` 当前操作失败但进程仍可继续、`fatal` 进程或 Session 已无法安全继续。
> - 每条记录必须给出稳定、可检索的 `msg` 和有界的 `scope`；结构化字段放 `data`，原始错误放 `err`（保留 `name`/`message`/`stack`/`cause`）。
> - **同一事实只由其拥有者记录**：纯透传层不重复记录；只有在不同层建立了不同语义事实时才产生多条日志。
> - 对于可能因多种原因失败的操作（如下载可能因网络、校验、磁盘空间失败），必须定义可区分的错误码，并说明该错误码是否跨契约边界暴露给调用方。
> - **数据边界是硬约束**：日志不得写入令牌、Cookie、密码、授权头、完整命令、stdin、协议 payload、用户输入、生成内容、请求体或本地文件内容。只有当前 Bus 源码或文档明确允许的有界身份与生命周期字段可以记录。
> - 计划必须写明该错误路径的验证入口（单元测试、集成测试或明确的开发模式观察）。

## 断言检测（如适用）

A private authenticated-self type prevents any caller from providing a recipient; only the auth parser creates it from the user identity. Always-on validation rejects empty/non-ou IDs and bot-only/unverified/invalid auth. Settings/control/CLI expose only an enable boolean and reject extra recipient/chat/group fields. Production uses exhaustive enum handling and explicit Result errors; unit and E2E tests assert every send's self recipient, user identity and [Bus] prefix, and none occur after failed enable/off.

> **重要**：
> - 本仓库**没有**共享断言辅助包。检查必须写在**最接近状态所有者的位置**并就地 fail-fast，保留原始原因。计划不得为此引入新的全局断言抽象，除非该抽象本身就是计划目标。
> - Rust 侧优先用类型和穷尽匹配表达不变量；确属不可恢复的内部状态时，在最近的状态所有者处 fail fast 并带上可定位上下文。
> - **生产环境必须守住的契约使用始终生效的检查，不依赖仅调试期的 `debug_assert!`**。仅调试期检查不能承担产品不变量。
> - **何时用断言**：「如果这里失败，说明上游代码有 bug」。**何时用错误日志**：「这里可能失败，但不是 bug，需要上报或恢复」。
> - 禁止用第二取值路径、猜测或默认值、额外 nullable、catch 后继续、静默 no-op 或自动重试绕过被违反的不变量；设计保证存在的值缺失时，应修复其唯一建立者。
> - 用 Rust 测试的 `assert!`、`assert_eq!`、`#[should_panic]` 或结果断言验证不变量；每条声明的断言都应在测试计划中有对应用例。

## 实施步骤

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
> - 主 agent 只在 task report evidence 有效、该任务验收通过且所属 wave isolation 通过时勾选 `**完成**`。主 agent 不逐任务重跑 gate/lint；任务完成复选框是共享工作树进度，不表示已 commit。
> - 自动 gate 完成前所有 actor 必须零 Git：不得 `git add/commit/push`。全部自动 gate 对最终计划 delta 全绿后，执行者以 `commit-and-push` 作为最后一步提交并推送到当前具名分支；此后不得再改树。随后交回人工验证、`pr` 与最终 `review-pr`。
> - 全局 EXIT CHECK 固定为 `just lint` → `just test` → 可选 `just build`，不得使用 task/file filter。任何修复改树都从 final lint 重新开始。任务级 Ready 与局部 review 对最终 PR review 不可替代。

### 任务1：Deliver the self-only developer-attention notification path

- [x] **完成**
- **目标**：The agreed notification behavior, runtime settings, mechanical sources and MASTER badge work together and are verified without touching live Bus or real Feishu.
- **拥有文件**：`src/messaging/`, `src/client/rooms/`, `src/client/compositor/tests/compositor_test.rs`, `src/cli/`, `src/terminal/state/`, `src/server/api/`, `src/protocol/api/schema/`, `src/agents/providers/codex/`, `src/devtools/tui/`, `tests/`, `tools/acceptance/`, `orchestration/`, `Cargo.toml`, `Cargo.lock`, `README.md`, `docs/bus-architecture.md`, `docs/tui-driver.md`, `plans/2026-10-18-feishu-notify.md`
- **blocked-by**：无
- **produces**：Self-only notifications from durable tagged orchestrator MASTER messages and proven mechanical events, live checkbox/CLI, guidance and regression evidence.
- **consumes**：无
- **工具**：Source and lark help evidence listed above, adjacent module test harnesses, scratch `bus --dev tui`, `just test-one`, existing schema golden generator.
- **参考实现**：Existing settings/control/sound UI, dialog episode stabilization, report promotion, stall_reason and quota freshness checks.
- **约束**：Run focused regressions RED before production changes, then same checks GREEN. Strict self-only recipient and [Bus] prefix. Never live Bus or real Feishu. One-shot integrated ownership; no parallel writers. Full gates only under common gate lock, use this worktree target, inspect complete failure logs.
- **验收闸门**：[TASK_LOCAL] Every new feature test in every owner includes `feishu_` or `developer_attention_` in its name; the dev-tools raw-reply directive test specifically uses the feishu_ prefix. `cargo test --locked --bin bus feishu_` and `cargo test --locked --bin bus developer_attention_` plus `cargo test --locked --features dev-tools --bin bus feishu_` run those exact newly added cases and pass (record names/counts, not merely nonzero existing tests). Regenerate with `BUS_UPDATE_API_SCHEMA=1 cargo test --locked --bin bus generated_protocol_schema_artifact_is_current`, then verify with the same command without that environment variable. Build the dev driver with `./run tui list`, then `python3 tools/acceptance/feishu_notify_test.py --binary target/tui-driver/debug/bus --run-name e2e-1` passes all scratch cases. Each command sets explicit worktree-local CARGO_TARGET_DIR and CARGO_INCREMENTAL=0; the runner also invokes CLI toggles without --dev against its scratch target.

Execution evidence and deviations will be appended here; source/test checks are sequential because they exercise one coupled state/delivery boundary.

Execution notes (2026-10-18):
- TDD evidence is retained under `temp/feishu-notify/`: real assertion REDs cover default settings/CLI/badge/durable state, auth parsing, subprocess lifecycle, self-only send, async runtime/delivery, mechanical latches/freshness/exit, original Codex timestamp, CLI auth waiting, the settings row, raw provider finals, retention, deleted quota observers, offline global enable generations, backlog visibility, quota-window recovery, toggle races and stall recovery. Corresponding focused GREEN logs name every case.
- `cargo test --locked --bin bus feishu_` passed 51 cases; `developer_attention_` passed 5, and feature-enabled `feishu_` passed 52 (including the dev-tools raw-final case). The combined feature/adjacent lifecycle filter passed 67 cases. The schema golden was regenerated and verified. Existing coordinator/settings compatibility regressions passed 190 cases. Final gate will revalidate the committed complete tree.
- The runtime projection uses `BusEvent::FeishuStatusChanged` plus local UI state and the existing snapshot error, avoiding a new `BusSnapshot` constructor contract. It exposes effective/pending/error plus durable backlog/rate state.
- A failing reopen test proved a shared boolean alone cannot distinguish ordinary restart from a global off/on cycle while a session was offline. The shared preference now carries an internal enable generation; a saved off-to-on transition advances it and each session stores its baseline generation. An explicit enable while already globally on preserves other sessions' backlogs. Recipient identity remains private and never persisted/configurable. This closes the reviewed off-period baseline contract without expanding the product scope.
- Fresh quota observations after the previous reset retire its episode; corrections to a still-future reset preserve the exhaustion episode. Positive Working observations re-arm later stalls. Unknown, missing and stale observations preserve latches. Retention prunes source guards with history and old agent/request episode keys. Session metadata and idempotency use the full data path: a RED regression proved that equal directory basenames otherwise collide across sessions.
- Scratch harness setup failures and exploratory runs are retained; completed exploratory reports `e2e-harness-03/report.md` and `e2e-error-clear/report.md` are GREEN. Final acceptance is rerun on the complete candidate before gate/review and again on final PR head before merge. Real Feishu receipt/phone alerts remain unrun; no real messages were sent.
- Complete-candidate scratch acceptance `temp/feishu-notify/e2e-1/report.md` passed all six steps; the checkbox, explicit-message/final/Bus badges were visually inspected, and the scratch session was stopped. The existing 15-second error toast is allowed to expire before screenshot capture. Gate/review/final-head acceptance remain the authorized delivery steps after this task-local completion.
- Gate round 1 caught three lint compatibility issues and two existing exact-inventory/settings assertions; all were repaired, with the same focused checks GREEN. Windows strict lint additionally reports pre-existing warnings in four unchanged files, attributed by base/HEAD blob equality in `temp/feishu-notify/windows-baseline.md`; it is not claimed GREEN. Plain Windows compilation and the required complete host gate are recorded separately. No Windows runtime or real Feishu receipt is claimed.
- Further assertion regressions cover shared-session enable/backlog, original quota observation ordering, fresh usage display without a reset, queued expiry across downtime, superseded versus new process exits, and CLI request correlation. The expiry model captures attention before settling work for both open and tick. Newest nonfuture Codex observations win; alert qualification remains stricter than usage display. A temporary unreadable settings file pauses delivery with a deduplicated visible/logged reason and resumes unattempted backlog after revalidation. Captured owner logs prove successful sends and effective transitions without message bodies or recipient/auth fields. Its subscriber is installed after the fixture lease so the same assertions also pass in parallel libtest.

## 测试计划

### 单元测试

- Transport tests with an executable stand-in: auth command shape, actual auth JSON variants including verified needs_refresh, missing binary/auth/scopes/self, malformed/oversized output, timeout, failed send, account change, stdin literal text, only resolved self recipient, explicit user sender, prefix and all metadata, no configurable recipient API. Controlled clock for rate window/dedupe; bounded queue backlog preserves unconsumed durable sources; disable cancels pending sends and never replays earlier sources. Failed/timeout sends invoke send exactly once, send-time auth unavailable/failure disables locally without retry, and delivery errors never recursively create notifications.
- Coordinator attention tests: tagged explicit MASTER post and final response each forward once; untagged progress, human, worker and internal work-room tags do not. New completed reply scan plus report copy cannot duplicate. Save failure emits no send. First enable skips its preexisting baseline; restart resumes unattempted backlog, never already attempted sources. Cover reopen with more than 128 sources, interrupted outcome-unknown attempts, retained rate/dedupe and explicit off/re-enable baseline semantics.
- Mechanical tests: stabilized orchestrator dialog once/rearm; worker dialog excluded; process exit marker exported and consumed only with same pane/terminal, missing/error alone excluded, intentional deletion/update excluded; fresh unexpired >=100% quota once/rearm, unknown/stale/expired excluded; existing queued and typed stalls once while healthy Working/Blocked/held-shell requests excluded.
- Model roundtrip retains tagged Bus notice, captured origin and mechanical latches; old untagged Bus notices still removed. Reopen under persistent exit/stall/dialog facts creates neither a duplicate notice nor a send. Startup with saved on, cross-session file on/off, observer auth failure isolation, pending off cancellation and re-enable to a newly resolved self all have coordinator tests.
- UI render tests for badge and stripped body in MASTER (both prompts and replies), untagged content unchanged; tagged human/worker text and work-room orchestrator finals get no badge; checkbox keyboard/mouse reflects pending/success/refusal without freezing delivery or settings. Require exact named send scopes and verify missing-scope refusal. CLI/control reject any alternate-recipient fields/flags.
- Concrete new tests live in `src/messaging/feishu/` and `src/messaging/coordinator/tests/attention_test.rs`; adjacent named source-cluster tests cover parser, model, settings, UI and native projection. No production module migration/deletion; constructor consumers only need the new false default, existing full unit/integration gates cover them.

### Scratch E2E

`tests/e2e-tui/feishu-notify.md` and `tools/acceptance/feishu_notify_test.py` define a fresh scratch session with isolated data/config and fake provider fixtures; fake lark-cli is the only executable resolved from PATH. Record all calls/stdin and assert every actual send uses exactly the authenticated fake self, never chat/group/bot/alternate user. Cover runtime UI on/off and public CLI on/off, missing binary/auth refusal, tagged orchestrator send, tagged final reply, untagged no-send, mechanical event, dedupe/rate and [Bus]+metadata. Save screenshots/cells for the visible badge/checkbox and trace plus call ledger. Run before review and on final PR head immediately before merge. Copy final evidence to `/Users/dylanliu/work/bus/temp/feishu-notify/` before worktree cleanup. No real smoke send is part of automated acceptance.

### 全局 EXIT CHECK

1. `just lint`
2. `just test`

The task owner's common lane sequence explicitly invokes gate-and-fix on a committed candidate, so its required clean committed input is authorized before that gate; the user lane process supersedes the template's generic zero-Git-before-gates/handoff instructions for this run. Run gate-and-fix (lint, unit, integration, coverage and diff) under the common gate lock on that committed candidate; its full lint, unit and integration commands implement these EXIT CHECK slots, without a duplicate uncommitted full test run. Use this worktree target. macOS runtime E2E is required. Run applicable Windows compile lint for platform-neutral process/snapshot changes if toolchain supports it; record any unavailable target evidence honestly. Full gate failures stop merge until attributed and resolved under the task rules.

> **重要**：
> - **自动化测试优先**：AI agent 可以运行 `just test-one <filter>`、`just test`、`just lint` 和适用的 Python/Bun 测试，因此脚本可执行的验证不归类为手动测试。
> - **手动测试仅限于最后手段**：只有终端交互、视觉流畅度、真实 shell/PTY 生命周期或当前不能安全自动化的 OS 行为才使用。运行命令和查看日志本身不是手动测试。
> - 测试按 Rust 单元/模块测试、维护脚本测试、集成资源测试和真实终端验收的最小充分层级放置。
> - 编写测试时遵守 [code-review-guide「Never pad coverage with a tautology」](../guides/code-review-guide.md#verification-boundary-prove-behavior-suspicions-with-tests-do-not-run-quality-gates)。
> - 跨平台逻辑使用 `cfg` 与对应 CI/目标验证；不要用运行时条件跳过来伪装覆盖。

> **重要**：
> - 前两步必需且顺序固定。`just build` 是可选的第三个槽位，仅在计划行为需要构建证明时保留。
> - 每个槽位只接受与当前 `justfile` 一致的完整仓库命令，不接受 task filter；计划特有的命令属于 `[TASK_LOCAL]` 闸门。
> - 需要 Windows 证明时，按影响范围额外运行 `just windows-lint` 或 CI 的 Windows gate，并如实记录当前宿主无法完成的真实终端交互。
> - 无法由 agent 完整运行的设备 / 视觉验收只进入「手动测试」。

### 手动测试（仅在无法自动化时使用）

- [ ] **Optional approved real self-delivery check**
  - Only after explicit task-owner approval of the concrete payload and user sending identity, enable on the final build and send one `[Bus]` smoke message to the currently authenticated self. No configurable recipient is introduced.
  - Confirm receipt and whether the developer's Feishu client raises an alert; those OS/client semantics cannot be proven by a stand-in. Required scopes are `im:message.send_as_user` and `im:message`.
  - This optional check is unrun unless approved and does not add a manual merge gate to the developer's specified automatic process. Report real receipt/phone-alert behavior as unproven when unrun.

<!-- 计划执行规则：开始 - 此部分不可修改 -->
## 计划执行规则

- AI 只以计划状态判断评审就绪：首次执行必须为 `review-plan-complete`，恢复执行允许为 `plan-execution-in-progress`。
- 执行者不再重复检查计划完整程度或逐文件审查状态；文件契约、任务 owner、依赖图与验收闸门仍必须通过结构和安全校验。
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
  - 每个任务必须发布与当前 generation、文件内容和 gate 日志绑定的 evidence，再经过执行者的任务验收。
  - 全部任务 Ready 后，在同一 candidate tree 上运行完整 EXIT CHECK；任何修复改树都从 final lint 重建证据。
  - 最终工作树必须包含任务 checkbox、deviation report 和 `plan-execution-complete` 状态，再计算 tree hash。
  - 全部 task Ready 后，执行者在 final gate 前写入 `plan-execution-complete`，使状态本身进入被验证 tree；任一 final gate 失败或 tree 漂移必须恢复 in-progress，修复后重新写入 complete 并从 final lint 重建证据。
  - agent-driven e2e 必须在该精确 tree 上运行；人工 e2e 只记录待验证 tree 与清单，不冒充通过。
  - 任务内不得出现 `(需要手动操作)`；人工验证只存在于「手动测试」section。人工结果若是后续实施的硬前置，必须拆成两份计划。
  - task acceptance 只是局部安全门；人工验证后仍必须运行完整 `/review-pr`。
  - XXL/XXXL 也使用同一粗粒度任务图；任务数量不按计划大小机械扩张。
  - 无需考虑渐进式迁移策略，应直接完整实现所需功能。
- **Git 落盘原则**（以 [`commit-and-push`](../../skills/commit-and-push/SKILL.md) 为 SOT）：
  - 从进入 `plan-execution-in-progress` 到全部自动 gate 完成，所有 actor 必须零 Git：不得 `git add`、`commit`、`push`、`stash`、`rebase` 或切换 ref。
  - 自动 gate 全绿后只允许以 `commit-and-push` 作为最后一步落盘，之后不得再改树。
  - 默认不在共享分支落盘；开发者明确要求直接在 `master` commit-and-push 时，该要求授权普通提交和显式 `git push origin master:master`，但不授权 force-push。
  - feature 分支落盘后交回开发者人工验证，再由 `pr` 创建或更新面向 `master` 的 PR，并由 `review-pr` 完成最终审查；执行者自身不 rebase、不创建 PR。
<!-- 计划执行规则：结束 -->
