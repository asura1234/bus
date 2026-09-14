# Task Agent Report 格式

本文是 `/execute-plan` task agent → 主 agent 中间产物的严格格式 SOT。完整证据写入
`report.md`；机械脚本校验后生成短 `completion.txt`，task agent 必须逐字返回后者。

## 文件位置

主 agent 为每个 task agent 分配唯一文件：

```text
<EXECUTION_STATE_ROOT>/attempts/attempt-NN/task-<task-id>/generation-NN/report.md
<EXECUTION_STATE_ROOT>/attempts/attempt-NN/task-<task-id>/generation-NN/completion.txt
```

task agent 只可写自己的 `report.md`、`completion.txt` 与当前 generation 的 `gate-NN.log`，
不得写其他 execution control state。这些文件不属于计划 `拥有文件`，也不授权 task agent 修改其他
owner 或控制面。

主 agent 在消费 report 后另行生成同目录 `file-scope.json`。它明确区分：

- `generation_delta_files`：本 generation 相对派发时刻真正改动的 owner 内文件；
- `cumulative_task_files`：本 task 从 execution/repair base 起累计形成的完整文件集合。

两者来自不同基线，互不要求包含；完整撤销上一代修改时 generation delta 可非空而 cumulative
为空。task agent 只报告前者，不得自行重建累计集合。`/review-task --files` 读取后者，并用
`--file-scope` 独立绑定两者和当前 report。

每次初次派发、`NEEDS_CONTEXT` 重触发或调整后的重派都必须由主 agent 分配下一个单调递增的
`generation-NN`。generation 路径是 turn identity：旧 generation 保留为证据，但不得覆盖、删除、
重新交付给 agent 或再次传给 waiter；当前 live wait set 只含每个 live task 的当前 generation。

## `report.md` 固定格式

标题、metadata 和 section 必须按下列顺序各出现一次：

```markdown
# Task Agent Report

- TASK_ID: 1
- TASK_NAME: 示例任务
- GENERATION: 1
- STATUS: DONE
- SNAPSHOT: temp/execute-plan/example/task-1-snapshot.md
- SNAPSHOT_SHA256: <64 位小写十六进制>
- TOUCHED_CONTENT_SHA256: <64 位小写十六进制>

## 实现摘要

完成 package-local test runner 接线。

## Touched files

- `cli_extensions/test_hooks.py`
- `cli_extensions/tests/test_test_hooks.py`

## Gate evidence

### Gate 1

- COMMAND: `just test-one <filter>`
- OWNER: task-agent
- LEVEL: TASK_LOCAL
- WORKDIR: `.`
- KIND: test
- EXIT_CODE: 0
- PASSED: 42
- FAILED: 0
- SKIPPED: 0
- COVERAGE: n/a
- LOG: `temp/execute-plan/example/attempts/attempt-01/task-1/generation-01/gate-01.log`
- LOG_SHA256: <64 位小写十六进制>
- RESULT: 42 passed

## Owner gap

无。

## Concerns

无。

## Context or blocker

无。
```

### Metadata

- `TASK_ID`：正整数，必须等于主 agent 派发的 task id。
- `TASK_NAME`：非空单行文本。
- `GENERATION`：正整数，必须与所在 `generation-NN` 目录一致。
- `STATUS`：只能是：
  - `DONE`
  - `DONE_WITH_CONCERNS`
  - `OWNER_GAP`
  - `NEEDS_CONTEXT`
  - `BLOCKED`
- `SNAPSHOT`：主 agent 交付的 task snapshot 路径，使用非空单行文本。
- `SNAPSHOT_SHA256`：snapshot 当前字节的 SHA-256。
- `TOUCHED_CONTENT_SHA256`：按规范路径排序后，绑定本 generation 每个 touched path 的
  file/symlink/deleted 身份与当前内容。用 renderer 同目录脚本的 `fingerprint` 子命令生成，不得手填。

### 实现摘要

说明完成了什么，或在未完成状态下说明尝试了什么。不得为空；确实没有实施时写 `无。`。

### Touched files

这里只列当前 generation 相对派发时刻的 delta，不重复列前一 generation 已验收且本轮未改的文件。
每行只能是一个反引号包裹的仓库根相对路径：

```markdown
- `path/to/file`
```

没有触及文件时必须且只能写：

```text
无。
```

不得混用 `无。` 与路径，不得使用绝对路径、`..` 或目录尾斜杠。

### Gate evidence

每个 gate 使用连续编号 `### Gate 1`、`### Gate 2`……，每项固定包含：

```markdown
- COMMAND: `<完整命令>`
- OWNER: task-agent
- LEVEL: TASK_LOCAL
- WORKDIR: `<仓库根相对 workspace | .>`
- KIND: test | coverage | typecheck | build | check
- EXIT_CODE: <整数>
- PASSED: <非负整数 | n/a>
- FAILED: <非负整数 | n/a>
- SKIPPED: <非负整数 | n/a>
- COVERAGE: n/a | statements=<0..100> branches=<0..100> functions=<0..100> lines=<0..100>
- LOG: `<当前 generation 目录内的 repo-relative gate-NN.log>`
- LOG_SHA256: <64 位小写十六进制>
- RESULT: <非空单行摘要>
```

`OWNER/LEVEL` 固定为 `task-agent/TASK_LOCAL`，不得把 main 的 `FINAL_TREE` 或其他 workflow
证据塞进 task report。没有运行 gate 时必须且只能写 `无。`。
`DONE` 与 `DONE_WITH_CONCERNS` 至少需要一个 gate，
且所有 `EXIT_CODE` 必须为 `0`。`test` / `coverage` 必须证明 `PASSED > 0`、`FAILED = 0`；
`coverage` 必须给出四项 summary，其他 kind 必须写 `COVERAGE: n/a`。机械 validator 会核对
snapshot 内反引号包裹的验收命令、workspace、snapshot/content/log hash、非零测试与 coverage
shape。主 agent 消费这些证据，不逐任务重跑 gate；最终组合树仍由 main 运行集中 `just lint`、`just test` 与计划声明的可选 `just build`。

完成态还必须恰有一条 `KIND: check` evidence，其 `COMMAND` 必须逐字等于：

```bash
python3 skills/execute-plan/scripts/task_scoped_lint.py \
  --repo <repo-root> [--file <generation-delta-file>]...
```

的 stdout。该脚本只选 generation delta 中当前仍存在的 Rust file/symlink，并生成排序、
shell-safe 的 `rustfmt --edition 2021 --check ...`；若 stdout 是 `NOT_APPLICABLE`，说明 touched
set 没有可 scoped-format 的 Rust 文件，不要求该 gate。类型闭合归 task 自己的窄测试/编译闸门，
整树 clippy 归 final lint。计划不得把 runtime scoped format 重复写进 task `验收闸门`。

task agent 先把 gate stdout/stderr 完整保存到 `gate-NN.log`，再运行：

```bash
python3 skills/execute-plan/scripts/task_agent_report.py fingerprint \
  --repo <repo-root> --snapshot <task-snapshot.md> \
  [--file <repo-relative-touched-file>]...
```

把 stdout 的两个 hash 逐字填入 metadata。每次修复必须分配新 generation、重跑受影响的 task
gate，并生成新的日志/hash/report；旧 generation 不得覆盖。

### Owner gap

没有 owner gap 时写 `无。`。`OWNER_GAP` 状态必须写：

```markdown
- PATH: `repo/relative/path`
- REASON: <为什么完成任务必须触及该路径>
```

`PATH` 遵守 touched file 的路径规则。其他状态禁止填写 owner gap。

### Concerns

- `DONE` 必须写 `无。`。
- `DONE_WITH_CONCERNS` 必须写非空疑虑，且不得写 `无。`。
- 其他状态可写疑虑或 `无。`。

这里记录已经完成但仍存在的正确性、范围或维护性疑虑。普通实现摘要不放在本节。

### Context or blocker

- `NEEDS_CONTEXT` 与 `BLOCKED` 必须写清缺少的信息、已经尝试的内容和需要主 agent
  采取的动作，不得写 `无。`。
- 其他状态必须写 `无。`。

## 状态语义

| 状态 | 含义 | 主 agent 动作 |
|------|------|---------------|
| `DONE` | 实现、自检和 task gate 已完成，无已知疑虑 | 验证 active-attempt isolation 并进入 task acceptance |
| `DONE_WITH_CONCERNS` | 已完成且 gate 全绿，但仍有明确疑虑 | 先裁决疑虑，再决定是否验收 |
| `OWNER_GAP` | 必要路径不在 owner，已在写入该路径前停止 | 进入 `owner-graph-contract` repair |
| `NEEDS_CONTEXT` | 缺少继续工作所需的确定信息 | 进入 `needs-context` repair |
| `BLOCKED` | 在当前契约和能力下无法完成 | 进入 `developer-decision` repair |

这些状态是闭集。`基本完成`、`大致可用`、`等待确认` 等自由文本不是合法状态。

## `completion.txt` 固定格式

`task_agent_report.py render-completion` 校验 `report.md` 后确定性生成：

```text
TASK_AGENT_COMPLETION
TASK_ID=1
STATUS=DONE
DELTA_FILES=2
GATES=1/1
CONCERNS=none
REPORT=temp/execute-plan/example/attempts/attempt-01/task-1/generation-01/report.md
```

规则：

- 固定 7 行并以换行结尾。
- `CONCERNS` 只能是 `none` 或 `present`。
- `DELTA_FILES` 是当前 generation delta 数量，不是 cumulative task footprint。
- `GATES=<通过数量>/<总数量>`。
- `REPORT` 使用调用脚本时传入的路径文本，不由模型改写。
- task agent 的最终消息必须与该文件逐字一致；不得添加寒暄、解释、Markdown 围栏或总结。
- renderer 以原子替换发布该文件；文件出现是 runtime-independent canonical 完成信号。宿主
  mailbox 通知只能提前唤醒主 agent，不能替代该 artifact。

主 agent 由 mailbox 唤醒，或通过下列有界等待得到 `READY=<absolute-completion-path>`：

```bash
python3 skills/execute-plan/scripts/task_completion_wait.py \
  --completion <COMPLETION_FILE> [--completion <OTHER_COMPLETION_FILE> ...] \
  --timeout-seconds 60
```

waiter 只检测当前 generation 是否已原子发布，不执行 task identity 校验。没有文件出现时输出
`TIMEOUT`；这不是 task 失败，不授权重派或伪造完成。出现文件后，主 agent 必须使用本 turn
派发时的同一 identity 参数运行：

```bash
python3 skills/execute-plan/scripts/task_agent_report.py validate \
  --repo <repo-root> --report <REPORT_FILE> --task-id <N> --task-name <唯一任务名称> \
  --snapshot <task-snapshot.md>
```

## Fail-closed

出现以下任一情况，脚本必须非零退出，task agent 不得报告完成：

- 标题、metadata 或 section 缺失、重复、乱序。
- 未知状态或 task id 不匹配。
- 状态与 owner gap、concerns、blocker 内容矛盾。
- 完成态没有 gate 或存在非零 gate。
- generation、snapshot/content/log hash、path、gate 编号或固定字段畸形。
- 完成态的 task contract gate 命令缺失、测试数为零、workspace/log 不存在、coverage evidence 缺失，或完成态
  scoped lint 未精确绑定 generation delta 中仍存在的文件。契约闸门命令只对 `DONE` / `DONE_WITH_CONCERNS`
  强制：`OWNER_GAP` / `NEEDS_CONTEXT` / `BLOCKED` 按定义在跑闸门之前就停下，在这些状态上仍要求契约命令
  会让「必须先报 repair」的唯一出口自身不可达。snapshot、内容与日志 hash 三项对全部状态一律强制。
- 空值没有使用唯一表达 `无。`。
