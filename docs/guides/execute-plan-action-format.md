# Execute-plan Action 格式

本文是 `execute_plan.py` 给主 agent 的短 action/status 中间产物格式 SOT。完整 state、report、
review、manifest 与 evidence 各自保留在原 artifact；action envelope 只携带一次 transition 所需的
最小身份，不复制第二套事实源。

`EXECUTION_STATE_ROOT` 固定为 `temp/execute-plan/<plan-stem>/`；driver 的 state、action、manifest、
report 与 final evidence 只能写入该 canonical root。

## Action envelope

```text
EXECUTE_PLAN_ACTION
ACTION_ID=<sha256>
EXPECTED_STATE_HASH=<sha256>
ACTION=<closed-set action>
TASK_ID=<positive integer>
GENERATION=<positive integer>
INPUT_ARTIFACTS=<comma-separated repo-relative paths | none>
EXPECTED_OUTPUT=<repo-relative path | none>
COMMAND=<single-line executable instruction>
```

字段固定为九行并以换行结尾。主 agent 必须把同一 envelope 的 `ACTION_ID` 与
`EXPECTED_STATE_HASH` 原样传回 `ingest-completion` 或 `ingest-review`；不得从 task id、路径或聊天
文本重建 identity。

合法 action：

- `DISPATCH_TASK`
- `REMEDIATE_TASK`
- `REVIEW_TASK`

同一 task 任意时刻最多一个 active action。每次 task implementation/remediation 使用新 generation；
旧 generation、旧 reviewer 或 resume 前迟到的 action 必须因 identity/state hash 不匹配而拒绝。
其他 task 的并行 transition 不应使本 task action 失效。

## Status envelope

无可立即执行的单 task action 时输出：

```text
EXECUTE_PLAN_STATUS
STATE=<WAITING | FINALIZE | preflight action>
NEXT=<single-line next action>
```

`status` 命令可额外输出 `PHASE`、`ATTEMPT`、`TASKS` 与 state artifact 路径。
完整路径列表、manifest JSON、测试/build stdout 不得进入默认输出。

Repair status 不伪装成可消费 action：

- `STATE=PLAN_REPAIR_REQUIRED`
- `REASON=upstream-contract | owner-graph-contract | needs-context | developer-decision`
- `upstream-contract` 额外输出唯一 `PRODUCER_TASK_ID`。

解决后统一调用 `resume-repair`。driver 创建 fresh attempt/generation；旧 attempt 只读保留。新
attempt 从最近已验证 base 派生，未验收 delta 继续可见。upstream repair 先只把可运行 producer
的比较基线重置到 execution base；未就绪 consumer 不进入 active owner 并集，待 producer Ready
后才在 fresh attempt 中把自己的比较基线重置到 execution base 并完整重验。

`STATE=FINALIZE` 的固定顺序是：

```text
run-final <计划声明的 gate 序列> -> finalize -> commit-and-push ->
verify-landing -> STATE=COMPLETE
```

首个 `run-final` 在声明/命令前置对账通过后机械写回 `plan-execution-complete`；任一 final gate
失败时恢复 `plan-execution-in-progress`，单纯查询 `next/status` 不得改写计划状态。
`commit-and-push` 必须是 execute-plan 的最后一步；它之后不得再运行会改树的动作。
`run-final` 必须亲自执行 canonical Bus `just` 命令，并把实际 worktree tree、exit code 与日志 hash
写入专用 final gate evidence；`finalize` 只消费当前 tree 上由 `run-final` 发布的成功记录。
新计划的 required gate 与 lint/build 形态由「全局 EXIT CHECK」声明唯一决定；finalizer
逐项对账，不接受 `--require-build` / `--manual-e2e` 省略或替换声明。兼容参数仅服务没有该 section
的旧计划。
canonical final gate 顺序为 `just lint`、`just test`、可选 `just build`。每个 kind 只接受一条
完整仓库命令，不允许 task/file filter。driver 将实际 argv、tree、exit code 与日志 identity
绑定到 gate evidence。终端交互和视觉验收绑定 committed tree 并记录为 manual verification，
不得冒充自动 gate。
`verify-landing` 在 commit-and-push 后校验
`HEAD^{tree}`、baseline 与远端同名分支，再输出 `STATE=COMPLETE`。

## Fail-closed

以下情况不得推进：

- action 不是当前 task 的 active action；
- generation、action id 或 expected state hash 迟到/不匹配；
- artifact path、plan/task identity 或 review mode 不匹配；
- active action 尚未消费却尝试分配不同 action。
