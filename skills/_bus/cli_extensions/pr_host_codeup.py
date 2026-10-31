"""`pr_host.py` 的 Codeup 实现：把 PR 操作翻译成 `aliyun devops codeup-*`。

平台差异的判断依据见 `docs/guides/codeup-pr-workflow.md`；这里只做机械翻译与 fail-closed 校验。
"""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path
from urllib.parse import quote

from pr_host_common import HostError, Remote


TOKEN_ENV = "ALIBABA_CLOUD_YUNXIAO_ACCESS_TOKEN"
ORG_ENV = "ALIBABA_CLOUD_YUNXIAO_ORGANIZATION_ID"

# Codeup 的创建 / 更新接口都没有 Draft 参数，详情接口也不返回 WIP 字段；唯一可写的载体是标题。
# 前缀是本仓约定的 Draft 标记，`view` 读回时剥掉，调用方看到的始终是逻辑标题。
WIP_PREFIX = "[WIP] "

OPEN_STATUSES = frozenset({"UNDER_DEV", "UNDER_REVIEW", "TO_BE_MERGED", "OPENED", "OPEN"})
STATE_BY_STATUS = {"MERGED": "MERGED", "CLOSED": "CLOSED"}

# commit status 与 check run 的取值统一成 pr_signals 的大写结论；未列出的完成态进 `other`。
STATUS_CONCLUSIONS = {
    "SUCCESS": "SUCCESS",
    "FAILED": "FAILURE",
    "FAILURE": "FAILURE",
    "ERROR": "ERROR",
    "CANCELED": "CANCELLED",
    "CANCELLED": "CANCELLED",
    "SKIPPED": "SKIPPED",
    "TIMED_OUT": "TIMED_OUT",
    "NEUTRAL": "NEUTRAL",
}
PENDING_STATES = frozenset({"PENDING", "RUNNING", "QUEUED", "IN_PROGRESS", "WAITING", "CREATED", ""})
# AI 评审运行期间先贴一条带此标记的占位评论，完成后原地改写成报告；它不是 finding，只是「评审未完成」的信号。
AI_REVIEW_PLACEHOLDER = "<!-- ai-review-placeholder-v1 -->"
MERGEABLE_BY_CONFLICT = {"NO_CONFLICT": "MERGEABLE", "HAS_CONFLICT": "CONFLICTING"}


def logical_title(raw: str) -> str:
    return raw[len(WIP_PREFIX) :] if raw.startswith(WIP_PREFIX) else raw


def _credentials(remote: Remote) -> tuple[str, str]:
    token = os.environ.get(TOKEN_ENV, "")
    if not token:
        raise HostError(f"缺少环境变量 {TOKEN_ENV}（云效个人访问令牌）；在调用前导出，不要写进命令行或仓库")
    # 组织 ID 就是 Codeup 仓库路径的第一段；环境变量优先，缺省时从 origin 推出。
    org = os.environ.get(ORG_ENV) or remote.path.split("/", 1)[0]
    return token, org


class CodeupHost:
    name = "codeup"

    def __init__(self, remote: Remote, runner=subprocess.run):
        self.remote = remote
        self._runner = runner
        self._token, self._org = _credentials(remote)
        self._repo_id: int | None = None

    def _call(self, command: str, *args: str) -> object:
        cmd = ["aliyun", "devops", command, "--organization-id", self._org, *args]
        env = {**os.environ, TOKEN_ENV: self._token}
        done = self._runner(cmd, capture_output=True, text=True, check=False, env=env)
        if done.returncode != 0:
            raise HostError(f"aliyun devops {command} 失败：{self._redact(done.stderr or done.stdout or '')}")
        text = (done.stdout or "").strip()
        try:
            # aliyun CLI 会把评论正文里的控制字符（如制表符）原样写进 JSON 字符串，严格模式会拒绝。
            return json.loads(text, strict=False) if text else None
        except json.JSONDecodeError as error:
            raise HostError(f"aliyun devops {command} 返回的不是 JSON：{self._redact(text)[:200]!r}") from error

    def _redact(self, text: str) -> str:
        # 所有来自外部进程的诊断文本都可能回显令牌，拼进异常之前统一抹掉。
        return text.strip().replace(self._token, "***")

    def repo_id(self) -> int:
        if self._repo_id is None:
            repo = self._call("codeup-get-repository", "--repository-id", quote(self.remote.path, safe=""))
            if not isinstance(repo, dict) or not isinstance(repo.get("id"), int):
                raise HostError(f"codeup-get-repository 没有返回数字仓库 ID：{self.remote.path}")
            self._repo_id = repo["id"]
        return self._repo_id

    def _mr_call(self, command: str, pr: str, *args: str) -> object:
        return self._call(command, "--repository-id", str(self.repo_id()), "--local-id", str(int(pr)), *args)

    def _get(self, pr: str) -> dict:
        mr = self._mr_call("codeup-get-change-request", pr)
        if not isinstance(mr, dict) or "localId" not in mr:
            raise HostError(f"合并请求 {pr} 不存在或返回结构缺少 localId")
        return mr

    def _source_patch_set(self, pr: str) -> dict:
        sets = self._mr_call("codeup-list-change-request-patch-sets", pr)
        sources = [s for s in sets or [] if s.get("relatedMergeItemType") == "MERGE_SOURCE"]
        if not sources:
            raise HostError(f"合并请求 {pr} 没有 MERGE_SOURCE 版本，取不到 HEAD")
        return max(sources, key=lambda s: s.get("versionNo", 0))

    def find(self, branch: str) -> list[dict]:
        repo_id, page, found = self.repo_id(), 1, []
        while True:
            rows = self._call(
                "codeup-list-change-requests",
                "--project-ids",
                str(repo_id),
                "--state",
                "opened",
                "--per-page",
                "100",
                "--page",
                str(page),
            )
            rows = rows or []
            found += [r for r in rows if r.get("sourceBranch") == branch and r.get("projectId") == repo_id]
            if len(rows) < 100:
                break
            page += 1
        return [
            {
                "number": r["localId"],
                "title": logical_title(r.get("title", "")),
                "url": r.get("detailUrl"),
                "isDraft": r.get("title", "").startswith(WIP_PREFIX),
            }
            for r in found
        ]

    def create(self, branch: str, base: str, title: str, body_file: str) -> dict:
        repo_id = str(self.repo_id())
        mr = self._call(
            "codeup-create-change-request",
            "--repository-id",
            repo_id,
            "--source-project-id",
            repo_id,
            "--target-project-id",
            repo_id,
            "--source-branch",
            branch,
            "--target-branch",
            base,
            "--title",
            WIP_PREFIX + title,
            "--description",
            Path(body_file).read_text(encoding="utf-8"),
        )
        if not isinstance(mr, dict) or "localId" not in mr:
            raise HostError("codeup-create-change-request 没有返回 localId")
        return {"number": mr["localId"], "url": mr.get("detailUrl"), "isDraft": True}

    def edit(self, pr: str, title: str, body_file: str) -> dict:
        current = self._get(pr)
        prefix = WIP_PREFIX if current.get("title", "").startswith(WIP_PREFIX) else ""
        body = Path(body_file).read_text(encoding="utf-8")
        self._mr_call("codeup-update-change-request", pr, "--title", prefix + title, "--description", body)
        return {"number": int(pr)}

    def view(self, pr: str) -> dict:
        mr = self._get(pr)
        status = (mr.get("status") or "").upper()
        if status in OPEN_STATUSES:
            state = "OPEN"
        elif status in STATE_BY_STATUS:
            state = STATE_BY_STATUS[status]
        else:
            raise HostError(f"合并请求 {pr} 的状态无法识别：{status!r}")
        raw_title = mr.get("title", "")
        return {
            "number": mr["localId"],
            "url": mr.get("detailUrl"),
            "title": logical_title(raw_title),
            "body": mr.get("description") or "",
            "headRefName": mr.get("sourceBranch"),
            "headRefOid": self._source_patch_set(pr).get("commitId"),
            "state": state,
            "baseRefName": mr.get("targetBranch"),
            "isDraft": raw_title.startswith(WIP_PREFIX),
            "mergedAt": mr.get("updateTime") if state == "MERGED" else None,
            "mergeCommit": mr.get("mergedRevision"),
        }

    def ready(self, pr: str) -> dict:
        raw = self._get(pr).get("title", "")
        if raw.startswith(WIP_PREFIX):
            self._mr_call("codeup-update-change-request", pr, "--title", logical_title(raw))
        return {"number": int(pr), "isDraft": False}

    def comment(self, pr: str, body_file: str) -> dict:
        body = Path(body_file).read_text(encoding="utf-8")
        # 作者自己发的整体评论（裁决台账）是审计记录，不是评审意见；以已解决状态创建，`comments` 才不会把它算成待处置。
        args = ["--comment-type", "GLOBAL_COMMENT", "--content", body, "--draft", "false", "--resolved", "true"]
        args += ["--patchset-biz-id", self._source_patch_set(pr)["patchSetBizId"]]
        self._mr_call("codeup-create-change-request-comment", pr, *args)
        return {"number": int(pr)}

    def request_review(self, pr: str) -> dict:
        self._mr_call("codeup-create-ai-review-run", pr)
        return {"number": int(pr), "reviewer": "codeup-ai-review"}

    def merge(self, pr: str, head: str, admin: bool, note: str | None) -> dict:
        if admin:
            raise HostError("Codeup 没有 admin 合并；非本 PR 造成的放行依据写进 --note-file 登记的“通过”备注")
        if not note or not note.strip():
            raise HostError("Codeup 必须先登记“通过”再合并，--note-file 不能为空")
        self._ensure_author_reviews(self._assert_mergeable_head(pr, head), pr)
        self._mr_call("codeup-review-change-request", pr, "--review-opinion", "PASS", "--review-comment", note)
        mr = self._assert_mergeable_head(pr, head)
        failing = [i for i in (mr.get("checkList") or {}).get("requirementRuleItems", []) if not i.get("pass")]
        if failing:
            raise HostError(f"登记通过后仍有未满足的合并卡点：{json.dumps(failing, ensure_ascii=False)}")
        title = logical_title(mr.get("title", ""))
        self._mr_call("codeup-merge-change-request", pr, "--merge-type", "squash", "--merge-message", title)
        merged = self.view(pr)
        if merged["state"] != "MERGED" or not merged["mergeCommit"]:
            raise HostError(f"合并命令返回后状态不是 MERGED：{merged['state']}")
        return merged

    def _ensure_author_reviews(self, mr: dict, pr: str) -> None:
        # 只有评审人能登记评审意见（否则 403 no access to submit review opinion）；“通过”由合并请求作者自己登记，
        # 所以先把作者加进评审人。接口按整份名单设置，必须带上已有评审人，不能顶掉别人。
        author = (mr.get("author") or {}).get("userId")
        if not author:
            raise HostError(f"合并请求 {pr} 缺少作者 userId，无法登记“通过”")
        reviewers = [r.get("userId") for r in mr.get("reviewers") or [] if r.get("userId")]
        if author in reviewers:
            return
        self._mr_call(
            "codeup-update-change-request-related-person", pr, "--type", "REVIEWER", "--user-ids", *reviewers, author
        )

    def _assert_mergeable_head(self, pr: str, head: str) -> dict:
        # API 没有 GitHub `--match-head-commit` 那样的原子锁；只能在每次写操作前后紧贴着核对。
        mr = self._get(pr)
        actual = self._source_patch_set(pr).get("commitId")
        if actual != head:
            raise HostError(f"合并请求 {pr} 的 HEAD 是 {actual}，不是待合并的 {head}")
        if mr.get("title", "").startswith(WIP_PREFIX):
            raise HostError(f"合并请求 {pr} 仍是 Draft（标题带 {WIP_PREFIX.strip()}）")
        if (mr.get("status") or "").upper() not in OPEN_STATUSES:
            raise HostError(f"合并请求 {pr} 不是打开状态：{mr.get('status')}")
        return mr

    def checks(self, pr: str) -> dict:
        mr = self._get(pr)
        patch_set = self._source_patch_set(pr)
        data = self._mr_call(
            "codeup-list-change-request-ci-check-list", pr, "--patch-set-biz-id", patch_set["patchSetBizId"]
        )
        if not isinstance(data, dict):
            raise HostError("codeup-list-change-request-ci-check-list 返回结构不是对象")
        entries = [_normalize_status(s) for s in data.get("commit_statuses") or []]
        entries += [_normalize_run(r) for r in data.get("check_runs") or []]
        pending = [c for c in entries if not c["completed"]]
        done = [c for c in entries if c["completed"]]
        failed = [c for c in done if c["conclusion"] in {"FAILURE", "ERROR", "TIMED_OUT"}]
        passed = [c for c in done if c["conclusion"] == "SUCCESS"]
        other = [c for c in done if c not in failed and c not in passed]
        conflict = (mr.get("conflictCheckStatus") or "").upper()
        mergeable = MERGEABLE_BY_CONFLICT.get(conflict, "CONFLICTING" if mr.get("hasConflict") else "UNKNOWN")
        return {
            "pr": mr["localId"],
            "head": patch_set.get("commitId"),
            "branch": mr.get("sourceBranch"),
            "base": mr.get("targetBranch"),
            "isDraft": mr.get("title", "").startswith(WIP_PREFIX),
            "mergeable": mergeable,
            # Codeup 落后目标分支时仍可 squash 合并，不像 GitHub 的 BEHIND 那样必须先同步。
            "mergeStateStatus": {"MERGEABLE": "CLEAN", "CONFLICTING": "DIRTY"}.get(mergeable, "UNKNOWN"),
            "behind": mr.get("behind"),
            "url": mr.get("detailUrl"),
            "ciDisplayType": data.get("display_type"),
            "checkCount": len(entries),
            "pending": sorted(c["name"] for c in pending),
            "passed": sorted(c["name"] for c in passed),
            "other": [{"name": c["name"], "conclusion": c["conclusion"]} for c in other],
            "failed": [
                {"name": c["name"], "workflow": None, "runId": None, "jobId": None, "url": c["url"], "logCommand": None}
                for c in failed
            ],
        }

    def comments(self, pr: str, output: str | None) -> dict:
        author = (self._get(pr).get("author") or {}).get("id")
        head = self._source_patch_set(pr).get("commitId")
        rows = [r for r in self._mr_call("codeup-list-merge-request-comments", pr) or [] if not r.get("is_deleted")]
        pending = any(AI_REVIEW_PLACEHOLDER in (r.get("content") or "") for r in rows)
        open_rows = [
            r
            for r in rows
            if not r.get("resolved")
            and AI_REVIEW_PLACEHOLDER not in (r.get("content") or "")
            and not _author_replied_last(r, author)
        ]
        if output:
            Path(output).parent.mkdir(parents=True, exist_ok=True)
            Path(output).write_text(render_comments(pr, head, open_rows), encoding="utf-8")
        return {
            "pr": int(pr),
            "head": head,
            "aiReviewPending": pending,
            "openCount": len(open_rows),
            "openIds": [r.get("comment_biz_id") for r in open_rows],
            "file": output if open_rows and output else None,
        }

    def metrics(self, pr: str) -> dict:
        mr = self._get(pr)
        author = (mr.get("author") or {}).get("id")
        rows = [r for r in self._mr_call("codeup-list-merge-request-comments", pr) or [] if not r.get("is_deleted")]
        reviews = [
            {"body": r.get("content") or "", "state": "COMMENTED"}
            for r in rows
            if r.get("comment_type") == "GLOBAL_COMMENT" and (r.get("author") or {}).get("id") != author
        ]
        threads = []
        for row in rows:
            if row.get("comment_type") == "INLINE_COMMENT":
                threads.append({"id": row.get("comment_biz_id"), "in_reply_to_id": None})
            for child in row.get("child_comments_list") or []:
                if not child.get("is_deleted"):
                    threads.append({"id": child.get("comment_biz_id"), "in_reply_to_id": row.get("comment_biz_id")})
        return {"info": self.view(pr), "reviews": reviews, "threads": threads}

    def reply(self, pr: str, comment_id: str, body: str) -> dict:
        # 平台要求回复与父评论同类型、同补丁版本；inline 父评论还要带同一处文件、行与对比版本。
        rows = self._mr_call("codeup-list-merge-request-comments", pr) or []
        parent = next((r for r in rows if r.get("comment_biz_id") == comment_id), None)
        if parent is None:
            raise HostError(f"合并请求 {pr} 上没有评论 {comment_id}")
        patch_set = (parent.get("related_patchset") or {}).get("patchSetBizId")
        if not patch_set:
            raise HostError(f"评论 {comment_id} 缺少关联补丁版本，无法回复")
        comment_type = parent.get("comment_type") or "GLOBAL_COMMENT"
        args = ["--comment-type", comment_type, "--content", body, "--draft", "false", "--resolved", "false"]
        args += ["--patchset-biz-id", patch_set, "--parent-comment-biz-id", comment_id]
        if comment_type == "INLINE_COMMENT":
            for flag, key in (
                ("--file-path", "filePath"),
                ("--line-number", "line_number"),
                ("--from-patchset-biz-id", "from_patchset_biz_id"),
                ("--to-patchset-biz-id", "to_patchset_biz_id"),
            ):
                if parent.get(key) is not None:
                    args += [flag, str(parent[key])]
        self._mr_call("codeup-create-change-request-comment", pr, *args)
        return {"number": int(pr), "commentId": comment_id}


def _author_replied_last(row: dict, author_id) -> bool:
    """作者的回复是这条评论下最新的一条，即作者已裁决、评审没有再追问。

    平台不允许合并请求作者把他人的评论标为已解决（`codeup-update-change-request-comment` 返回 403），
    所以「已裁决」只能由回复本身表达；评审在作者回复后再追问，这条评论重新算作待处置。
    """

    children = [c for c in row.get("child_comments_list") or [] if not c.get("is_deleted")]
    if author_id is None or not children:
        return False
    latest = max(children, key=lambda c: c.get("comment_time") or "")
    return (latest.get("author") or {}).get("id") == author_id


def _normalize_status(entry: dict) -> dict:
    state = (entry.get("state") or entry.get("status") or "").upper()
    return {
        "name": entry.get("context") or entry.get("name") or "(unnamed status)",
        "completed": state not in PENDING_STATES,
        "conclusion": STATUS_CONCLUSIONS.get(state, state),
        "url": entry.get("target_url") or entry.get("targetUrl"),
    }


def _normalize_run(entry: dict) -> dict:
    status = (entry.get("status") or "").upper()
    conclusion = (entry.get("conclusion") or "").upper()
    return {
        "name": entry.get("name") or "(unnamed check)",
        "completed": status == "COMPLETED",
        "conclusion": STATUS_CONCLUSIONS.get(conclusion, conclusion),
        "url": entry.get("details_url") or entry.get("detailsUrl"),
    }


def render_comments(pr: str, head: str | None, rows: list[dict]) -> str:
    """把未解决评论原文渲染成 address-review-comments 的 free-form 输入；不做任何改写或归纳。"""

    lines = [f"# Codeup 合并请求 !{int(pr)} 未解决评论", "", f"- HEAD：`{head}`", f"- 条数：{len(rows)}", ""]
    for row in rows:
        where, line = row.get("filePath"), row.get("line_number")
        patch = (row.get("related_patchset") or {}).get("commitId") or ""
        lines += [
            f"## 评论 {row.get('comment_biz_id')}",
            "",
            f"- 作者：{(row.get('author') or {}).get('name', '')}",
            f"- 类型：{row.get('comment_type')}",
            f"- 位置：{where}:{line}" if where else "- 位置：整体评论",
            f"- 版本：`{patch[:8]}`{'（已过期）' if row.get('out_dated') else ''}",
            "",
            (row.get("content") or "").rstrip(),
            "",
        ]
        for child in row.get("child_comments_list") or []:
            lines += [
                f"> 回复（{(child.get('author') or {}).get('name', '')}）：{(child.get('content') or '').strip()}",
                "",
            ]
    return "\n".join(lines).rstrip() + "\n"
