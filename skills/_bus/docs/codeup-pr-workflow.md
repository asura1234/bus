# Codeup PR 创建与合并

当目标仓库的 `origin` 为 Codeup 时，`pr` / `merge-pr` 的流程两端相同，平台操作统一经
`<skills dir>/_bus/cli_extensions/pr_host.py`：它按 `git remote get-url origin` 选择 `gh` 或 `aliyun devops codeup-*`，
未知主机直接失败。本文只记录 Codeup 与 GitHub 不同、需要 agent 理解的地方；命令翻译与 fail-closed 校验
以脚本及其测试（`<skills dir>/_bus/cli_extensions/tests/pr_host_test.py`）为准，不要手工拼 `aliyun` 命令替代它。

## 环境

- CLI：`aliyun` + `aliyun-cli-devops` 插件；参数以 `aliyun devops <cmd> --help` 为准，按
  `alibabacloud-cli-guidance` skill 准备。
- 凭据：调用前在环境里导出 `ALIBABA_CLOUD_YUNXIAO_ACCESS_TOKEN`（云效个人访问令牌）；
  `ALIBABA_CLOUD_YUNXIAO_ORGANIZATION_ID` 缺省时取仓库路径第一段。脚本只经子进程环境传递令牌，
  报错时抹掉令牌；令牌不得出现在命令行、PR 正文、评论、日志或仓库文件里。
- Git over HTTPS 需要凭据时，按命令临时注入（例如一次性 `credential.helper`），不写入 git config。
  用 `python3 "<skills dir>/worktree-new/scripts/repo_profile.py" policy --repo "<repo>"`
  读取 `policy.submodule_update`；为 `none` 时 fetch 加 `--no-recurse-submodules`，为 `recursive` 时按该策略更新子模块。

## 平台映射

| 概念 | GitHub | Codeup |
|------|--------|--------|
| PR 身份 | PR number | 合并请求 `localId`（仓库内序号）；创建需要数字仓库 ID，由 `codeup-get-repository` 解析一次 |
| HEAD | `headRefOid` | 最新 `MERGE_SOURCE` 补丁版本的 `commitId` |
| Draft | `--draft` / `gh pr ready` | 标题前缀 `[WIP] ` / `ready` 去掉前缀 |
| 可合并 | `mergeable` + `mergeStateStatus` | `conflictCheckStatus`；落后目标分支仍可 squash，不映射为 `BEHIND` |
| CI | `statusCheckRollup` | 最新补丁版本的 `codeup-list-change-request-ci-check-list` |
| 评审评论 | GitHub review lane | 未解决、且最新回复不是作者的合并请求评论；处置即作者回复 |
| 机器评审 | Copilot | Codeup AI 评审（`codeup-create-ai-review-run`） |
| 合并 | `gh pr merge --squash --match-head-commit` | 登记“通过”（PASS）→ squash，前后核对 HEAD |

**Draft 为什么用标题前缀**：`codeup-create-change-request` 与 `codeup-update-change-request` 都没有
Draft / WIP 参数，`codeup-get-change-request` 的详情也不返回 WIP 字段；列表里的 `workInProgress` 只读。
标题是唯一可写、所有接口都读得到的载体，因此 `[WIP] ` 是本仓的 Draft 标记：`view` / `find` 读回时剥掉前缀，
调用方看到的始终是逻辑标题与 `isDraft`；`edit` 保留现有前缀，不会顺手改变 Draft 状态。
实测 Codeup 原生识别该前缀：带 `[WIP] ` 创建后状态为 `UNDER_DEV`、列表 `workInProgress == true`，去掉前缀即退出 WIP。
不要在标题上另加其他 WIP 写法。

平台会去掉正文末尾的换行；读回比较时忽略末尾换行，其余内容必须逐字一致。

## 先通过，再合并

**Codeup 必须先登记“通过”（`PASS`），再执行合并。** 直接对“评审中”状态调用 merge，即使没有冲突，也可能
返回 `405 / SYSTEM_FORBIDDEN_ERROR`：`该状态下的评审不允许合并`；页面显示“评审通过人数不足”。
`pr_host merge` 把“核对 HEAD → PASS → 再核对 HEAD 与合并卡点 → squash → 读回 MERGED”做成一步，
任何一步不符即停止。只有评审人能登记评审意见（否则 `403 no access to submit review opinion`），所以登记前若作者
不在评审人名单里，脚本把作者追加进去（保留已有评审人）；这是平台登记“通过”的前提，不是冒用他人身份。

- 用户仅要求开 PR 时，不登记通过、不合并。
- 用户要求合并时，完成 `merge-pr` 的评审处置并记录 CI 快照后合并；CI 不作为合并前置条件。
  PASS 备注（`--note-file`）写明实际本地验证、评审处置结果和 CI 状态。开发者明确要求忽略 review 时，
  如实记录授权，不得宣称已完成独立评审；未通过或未执行的 CI 也不得写成通过。
- Codeup 没有 `--admin`；按既有权限登记 PASS 并合并，平台拒绝时报告原因。
- 放行只作用于本次 PR。不要为此修改仓库或保护分支规则，不冒用其他评审人身份；当前账号不允许登记通过或合并时，
  保留 PR 并报告平台拒绝原因。
- API 没有 `--match-head-commit` 等价物，不能声称有 GitHub 同等的原子版本锁；脚本只能在写操作前后紧贴着核对。
  合并后 fetch，确认 `origin/<base_branch>` 包含读回的 `mergeCommit`（Codeup `mergedRevision`）。

## CI

本仓 CI 仅作记录，不要求通过、不等待检查注册或完成，也不因缺少检查、运行中、失败、取消或状态
不可用再次征求放行授权。本地开发验证仍按 `pr` / `gate-and-fix` 执行，并如实记录结果。

Codeup 未挂合并请求 CI 时，`checks` 可能返回 `checkCount == 0` 与
`ciDisplayType == DEFAULT_PAGE_NO_RUN_RECORD`，记录为“没有平台 CI 信号”，不声明 CI 通过。
不需要逐 PR 授权以本地门禁替代 CI；此策略不改变平台权限、保护分支规则或 HEAD 核对要求。

## 评审评论

Codeup 评论没有 GitHub lane 的轮次 / anchor 机制。收敛判据：**待处置评论为零**——待处置指未解决、未删除、
不是 AI 评审占位，且最新一条回复不是合并请求作者的评论。平台不允许作者把他人的评论标为已解决（更新评论返回
403），所以作者的回复本身就是“已裁决”的记录；评审在作者回复后追问，该评论重新算作待处置。
`pr_host comments` 把待处置评论原文渲染成 free-form 评审文件，交 `address-review-comments pr --free-form-file`
裁决（锁定目标取 `pr` skill 写下的 `.locked-goal`）；裁决后对每条评论 `pr_host reply`，回复写明 disposition
与一手证据（APPLY 写修复 commit）。回复会挂在父评论的类型与补丁版本上，inline 评论带同一处文件与行，这是平台
的硬性要求。REJECT 同样回复——作者裁决过即收敛，不要求评审同意。台账全文用
`pr_host comment` 贴回合并请求，理由同 GitHub：REJECT 不留痕，机器评审下一轮会原样再提。

Codeup AI 评审（云效 AI 助手）由 `pr_host request-review` 触发（创建合并请求不会自动触发）。运行期间它先贴一条
带 `<!-- ai-review-placeholder-v1 -->` 的占位评论，完成后原地改写为报告，并逐条发 inline 评论；`comments` 把占位
评论报告为 `aiReviewPending` 而不当作 finding。它只评审触发时的补丁版本，修复推送后需要重新 `request-review`。
报告与“变更概述”两条整体评论同样待处置：按 HOUSEKEEPING 回复即可，其中的问题以 inline 评论为准。
`codeup-sync-ai-review-run` 在当前插件版本返回 404，不作为状态来源。

## 交付

给出 Codeup 合并请求链接、源 HEAD 与合并提交，明确实际验证结果以及按开发者指令跳过的检查。报告、截图与本地
审计产物仍遵循根 `AGENTS.md` 的入仓规范。
