#!/usr/bin/env python3
"""PR 合并前的 CI 状态与变更范围快照。

本脚本只拉取并分组平台事实；CI 状态仅供记录，不作为合并要求。

子命令：
  checks   --pr N                本次 HEAD 的全部 check 结论；失败项附带证据指针
  scope    --pr N [--base REF]   本 PR 触及的顶层目录与文件集
  history  --pr N --job NAME     该 job 在本分支历次 HEAD 上的结论（按需查询）

平台由 `cli_extensions/pr_host.py` 按 `origin` 判定：GitHub 走下面的 `gh` 实现；Codeup 的
`checks` 由 pr_host 取最新补丁版本的 CI 检查列表、输出同一形状，`scope` 同样适用，`history`
没有平台事实源，报告不可用，不影响合并。

评审评论归 `address-review-comments` 处置，不在此脚本中处理。
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path


sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "_bus" / "cli_extensions"))

import pr_host  # noqa: E402


class SignalError(RuntimeError):
    """拿不到信号（gh 不可用、PR 不存在、字段缺失）。"""


def _gh(*args: str) -> str:
    result = subprocess.run(["gh", *args], capture_output=True, text=True, check=False)
    if result.returncode != 0:
        raise SignalError(f"gh {' '.join(args)} 失败：{result.stderr.strip()}")
    return result.stdout


def _git(*args: str) -> str:
    result = subprocess.run(["git", *args], capture_output=True, text=True, check=False)
    if result.returncode != 0:
        raise SignalError(f"git {' '.join(args)} 失败：{result.stderr.strip()}")
    return result.stdout


DETAILS_URL_RE = re.compile(r"/actions/runs/(\d+)(?:/job/(\d+))?")

# rollup 的 conclusion 取值不止成功/失败两种，而**除 SUCCESS 外都不是绿**。
# 明确列出失败类，其余非 SUCCESS 的（CANCELLED / SKIPPED / NEUTRAL / STALE…）一律进 `other`
# 保留原始结论供报告，不把「不在失败名单里」写成通过。
FAILURE_CONCLUSIONS = frozenset({"FAILURE", "TIMED_OUT", "STARTUP_FAILURE", "ACTION_REQUIRED", "ERROR"})


def pr_view(pr: str) -> dict:
    data = json.loads(
        _gh(
            "pr",
            "view",
            pr,
            "--json",
            "number,headRefName,headRefOid,baseRefName,state,isDraft,mergeable,mergeStateStatus,url,statusCheckRollup",
        )
    )
    if not data.get("headRefOid"):
        raise SignalError(f"PR {pr} 没有 headRefOid")
    return data


def _normalize_check(entry: dict) -> dict:
    """把 rollup 的两种条目统一成一个形状。

    Actions 的是 `CheckRun`（有 name / workflowName / status / conclusion），
    外部状态是 `StatusContext`（只有 context / state，没有 status 维度）。
    """

    if entry.get("__typename") == "StatusContext":
        state = (entry.get("state") or "").upper()
        return {
            "name": entry.get("context") or "(unnamed status)",
            "workflow": None,
            "completed": state not in ("PENDING", "EXPECTED", ""),
            "conclusion": state,
            "url": entry.get("targetUrl"),
            "runId": None,
            "jobId": None,
        }

    match = DETAILS_URL_RE.search(entry.get("detailsUrl") or "")
    return {
        "name": entry.get("name") or "(unnamed check)",
        "workflow": entry.get("workflowName"),
        "completed": (entry.get("status") or "").upper() == "COMPLETED",
        "conclusion": (entry.get("conclusion") or "").upper(),
        "url": entry.get("detailsUrl"),
        "runId": int(match.group(1)) if match else None,
        "jobId": int(match.group(2)) if match and match.group(2) else None,
    }


def _log_command(check: dict) -> str | None:
    if check["jobId"]:
        return f"gh run view --job {check['jobId']} --log-failed"
    if check["runId"]:
        return f"gh run view {check['runId']} --log-failed"
    return None


def checks(pr: str) -> dict:
    """本次 HEAD 的全部 check 结论，逐**条**（不是逐 workflow）。

    事实源是 `statusCheckRollup`，也就是 GitHub 自己用来判定能否合并的那份。不要改回
    `gh run list`：后者是 workflow 粒度，本仓一个 `Lint Check` workflow 下挂着 `Glossary Lint`
    / `RN Unit Test` / `TS Lint` 三条独立的 check（`Build Check` 下同理），塌缩成一个名字
    之后，归因赖以成立的「哪个平台的哪个 job 红了」就没了；它同时还会列出 `Copilot` 这种
    根本不参与 gating 的 workflow。两个方向都错，一个报少一个报多。
    """

    info = pr_view(pr)
    rollup = [_normalize_check(e) for e in (info.get("statusCheckRollup") or [])]

    pending = [c for c in rollup if not c["completed"]]
    done = [c for c in rollup if c["completed"]]
    failed = [c for c in done if c["conclusion"] in FAILURE_CONCLUSIONS]
    passed = [c for c in done if c["conclusion"] == "SUCCESS"]
    other = [c for c in done if c["conclusion"] not in FAILURE_CONCLUSIONS and c["conclusion"] != "SUCCESS"]

    return {
        "pr": info["number"],
        "head": info["headRefOid"],
        "branch": info["headRefName"],
        "base": info["baseRefName"],
        "isDraft": info.get("isDraft"),
        "mergeable": info.get("mergeable"),
        "mergeStateStatus": info.get("mergeStateStatus"),
        "url": info.get("url"),
        "checkCount": len(rollup),
        "pending": sorted(c["name"] for c in pending),
        "passed": sorted(c["name"] for c in passed),
        "other": [{"name": c["name"], "conclusion": c["conclusion"]} for c in other],
        "failed": [
            {
                "name": c["name"],
                "workflow": c["workflow"],
                "runId": c["runId"],
                "jobId": c["jobId"],
                "url": c["url"],
                "logCommand": _log_command(c),
            }
            for c in failed
        ],
    }


def scope(pr: str, base: str | None, info: dict | None = None) -> dict:
    """本 PR 实际触及的目录与文件。"""

    info = info or pr_view(pr)
    base_ref = base or f"origin/{info['baseRefName']}"
    merge_base = _git("merge-base", base_ref, info["headRefOid"]).strip()
    files = [f for f in _git("diff", "--name-only", f"{merge_base}...{info['headRefOid']}").splitlines() if f]
    return {
        "pr": info["number"],
        "base": base_ref,
        "mergeBase": merge_base,
        "head": info["headRefOid"],
        "fileCount": len(files),
        "topLevel": sorted({f.split("/")[0] for f in files}),
        "files": files,
    }


def _run_jobs(run_id: int) -> list[dict]:
    data = json.loads(
        _gh(
            "api",
            f"repos/{{owner}}/{{repo}}/actions/runs/{run_id}/jobs?per_page=100",
        )
    )
    return data.get("jobs", [])


def history(pr: str, job: str, limit: int = 12) -> dict:
    """同一条 check 在本分支历次 HEAD 上的结论。

    `job` 是 `checks` 给出的**逐条 check 名**（如 `Android Debug Build`），不是 workflow 名。
    历史必须同样按条取：workflow 粒度的历史会把同一个 workflow 下几条 check 的成败混成一个
    结论，于是「这条以前就红」和「这条是我搞红的」再也分不开——而那正是本命令唯一的用途。
    """

    info = pr_view(pr)
    rollup = [_normalize_check(e) for e in (info.get("statusCheckRollup") or [])]
    owning_workflow = next((c["workflow"] for c in rollup if c["name"] == job and c["workflow"]), None)

    runs = json.loads(
        _gh(
            "run",
            "list",
            "--branch",
            info["headRefName"],
            "--limit",
            "100",
            "--json",
            "databaseId,headSha,workflowName,status,conclusion,createdAt",
        )
    )
    candidates = [r for r in runs if r["status"] == "completed"]
    if owning_workflow:
        candidates = [r for r in candidates if r["workflowName"] == owning_workflow]
    candidates.sort(key=lambda r: r["createdAt"], reverse=True)
    candidates = candidates[:limit]

    rows = []
    for run in candidates:
        for entry in _run_jobs(run["databaseId"]):
            if entry.get("name") != job:
                continue
            rows.append(
                {
                    "sha": run["headSha"][:8],
                    "conclusion": (entry.get("conclusion") or "").upper(),
                    "runId": run["databaseId"],
                    "jobId": entry.get("id"),
                    "createdAt": run["createdAt"],
                    "isCurrentHead": run["headSha"] == info["headRefOid"],
                }
            )
    rows.sort(key=lambda r: r["createdAt"])

    return {
        "job": job,
        "workflow": owning_workflow,
        "head": info["headRefOid"],
        "runsInspected": len(candidates),
        "runs": rows,
    }


def codeup_signals(host, args: argparse.Namespace) -> dict:
    if args.command == "checks":
        return host.checks(args.pr)
    if args.command == "scope":
        return scope(args.pr, args.base, host.view(args.pr))
    raise SignalError("Codeup 不提供逐条 check 的历史结论；CI 仅作记录，不影响合并")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    c = sub.add_parser("checks", help="本次 HEAD 的全部 check 结论")
    c.add_argument("--pr", required=True)

    s = sub.add_parser("scope", help="本 PR 触及的文件与顶层目录")
    s.add_argument("--pr", required=True)
    s.add_argument("--base", default=None)

    h = sub.add_parser("history", help="某条 check 在本分支历次 HEAD 上的结论")
    h.add_argument("--pr", required=True)
    h.add_argument("--job", required=True, help="checks 给出的逐条 check 名，不是 workflow 名")
    h.add_argument("--limit", type=int, default=12, help="最多回溯多少次 workflow run（默认 12）")

    args = parser.parse_args(argv)
    try:
        host = pr_host.detect()
        if host.name == "codeup":
            payload = codeup_signals(host, args)
        elif args.command == "checks":
            payload = checks(args.pr)
        elif args.command == "scope":
            payload = scope(args.pr, args.base)
        else:
            payload = history(args.pr, args.job, args.limit)
        print(json.dumps(payload, ensure_ascii=False, indent=2))
        return 0
    except (SignalError, pr_host.HostError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
