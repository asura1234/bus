# Two M4 minis as CI — ideas, not a spec

**状态**：ideas only（未排期、未承诺）
**作者**：dylanliu8949
**创建日期**：2026-11-08

> 这是一份想法记录，不是计划、不是规格、不是承诺。它记录我们想把两台闲置的 M4 Mac mini 用成 Bus 的 CI 之后应该长成什么样，以及动手前必须想清楚的事。和 [multi-machine ideas](2026-11-08-multi-machine-ideas.md) 不同：多机 Bus 是产品方向，这两个 mini 只是开发基础设施。

## 为什么

今天的合并门禁是一次本地 `gate-and-fix`：实测完整 gate 约 1066–1224 秒（18–20 分钟），而且 `suite_run.py` 的 lane 是一条一条串行跑的（lint → instrumented build → unit → integration → python → dev-tools）。它同时跑在还要跑 libtv 和所有 agent 构建的同一台机器上。

两个目标：

1. **最大并行**：多个 PR 同时在各自机器上 gate，而不是排队等同一台机器。
2. **让 agent 诚实**：现在 gate 由 agent 在自己的 worktree 里跑，"我 gate 过了"无法被独立验证，也出现过 gate 的是没 push 的本地 head。CI 把通过/失败绑到 pushed SHA 上，成为唯一的合并权威。

## 硬件

- 两台 M4 Mac mini：16GB 和 24GB。
- 一台机器一个 runner、一次一个 job：gate 是 CPU 密集的，同机两个 job 只会互相拖慢，而不是提高吞吐。

## 形态

- **repo 级的 self-hosted GitHub Actions runner**，两台都打 `self-hosted, macOS, ARM64, gate` 标签；GitHub 把 job 排给空闲的那台。
- 为什么 self-hosted：机器是自己的、没有队列等待、能装 llvm-cov/sccache，也不用为 macOS runner 付分钟数。
- 为什么不用自定义 SSH 派发当主路径：那样只是"更多电脑"，PR 上没有 check，merge 流程还得自己写派发和状态回报。checks 挂在 PR 上之后，读 CI conclusions 的合并流程可以直接复用。
- 不引入外部 CI 服务（Buildkite 之类）：和 multi-machine 的方向一致，机器在自己手里。

## 角色

- **24GB → 主 runner**：`CARGO_BUILD_JOBS`≈8、nextest workers≈8。llvm-cov instrumented build 吃内存，这台更从容。
- **16GB → 第二 lane**：workers≈6，避免 instrumented build + 并行测试触发 swap。

## 诚实性（核心）

- 合并权威 = 该 PR 在 **exact head SHA** 上的绿色 CI check，不是 agent 的自述。
- 本地 gate 降级为快速迭代：只跑 affected tests（2–5 分钟），不再作为合并依据。
- **gate 实现入库**（例如 `scripts/gate/`），CI 和 agent 跑同一份代码，不会出现 skill 目录和 repo 各一份、慢慢漂移的情况。
- 报告里记录 toolchain、gate script revision、runner 名 —— 版本漂移在每次 check 上都可见。
- 副作用：不再存在"gate 一个没 push 的本地 head"。

## 工作流

- 触发：`pull_request` 的 opened / ready_for_review / synchronize，加 `workflow_dispatch`。
- `concurrency: pr-<number>` + `cancel-in-progress: true`：修复 agent 连续 push 时只 gate 最新 head。
- artifact：完整 run JSON + 失败输出，agent 失败后下载它再本地修。
- **公开 repo 的限制**：self-hosted runner 绝不在 fork PR 上执行。workflow 只 gate 同 repo 分支（`github.event.pull_request.head.repo.full_name == github.repository`），fork PR 需要 maintainer 批准；我们自己的 PR 都来自同 repo 分支。

## 性能

- `sccache` + 每台持久 `CARGO_TARGET_DIR`：每个分支都要重编同样的依赖，这是单项最大收益。
- 四个 wall-clock flaky 测试保持排除，直到事件驱动的修复落地；mini 上只有 CI、没有别的负载，这几个测试可能反而变稳，届时再逐个恢复。
- 吞吐估算：M4 上一次 gate 约 20–25 分钟；两台 ≈ 5–6 gates/hour，匹配当前 2–4 个待 gate PR 的稳态。再多 runner 只会闲置。

## 权限与接入

- `asura1234/bus` 是公开 repo，我们的 GitHub 身份是 WRITE 不是 ADMIN，拿不到 runner registration token（403）。
- 需要：两台 mini 的 SSH 可达（host/user）+ 开发者注册 runner，或给 admin 权限。
- self-hosted runner 在公开 repo 上是有供应链风险的，规则见「工作流」一节。

## 风险

- **版本漂移**：toolchain、gate 脚本、Python 依赖都要在每台固定，并写进 artifact。
- **时序测试**：不同硬件和负载下的 wall-clock 行为可能不同；先观测，再决定恢复哪几个。
- **磁盘**：target 目录 + sccache 每台要 50–100GB。
- **runner 维护**：系统更新、服务重启、掉线要能被发现；dashboard 的 node 维度以后可以覆盖这一层。

## 如果先试一台

1. 24GB 先装 runner：工具链、nextest、llvm-cov、Python 依赖、warm target、sccache。
2. gate 脚本入库并写 `gate.yml`，走正常 PR 流程合入。
3. 同一个 head 在本地和 CI 各 gate 一次，比较耗时和结果，再决定是否信任。
4. 信任后加 16GB 那台，然后把合并策略切到「绿色 CI check 才允许合并」。

## 不做

- 不做 GitHub 托管的 macOS runner。
- 不把一次 gate 的 lane 拆到多台机器上（协调成本大于收益；先做多 PR 并行）。
- 不把 e2e 的判断搬进 CI：TUI 的"像人一样点"仍由 agent 做，CI 只负责 full gate。
- 不让 mini 跑 agent：它们只做 CI。

## 开放问题

- WIP push 是否触发：draft 不触发、ready 触发，还是每个 synchronize 都触发？
- stacked train 只 gate tip，还是每个 child 都 gate？
- 非 tip 的 PR 是否只跑 affected tests（便宜），full gate 留给 tip？
- artifact 保留多久？
- `merge-pr` / `gate-and-fix` 如何消费远程 check：push 后等待 check，失败下载 artifact 再本地修。
- 这两台以后是否顺带跑一个 Bus room（比如专门做构建/测试的 room），还是永远只做 CI？
