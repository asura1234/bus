#!/usr/bin/env python3
"""The two pre-merge signals of a PR: CI conclusions and review comments.

This script does only the mechanical part — fetching, grouping, slicing by round, and computing "does the failure fall
inside this PR's change scope" **as fact first** before handing it to the agent to judge. The attribution judgment itself stays with
the agent (see guide.md), but it must be made with the evidence computed here, not guessed from job names or log tails.

Subcommands:
  checks   --pr N                every check conclusion for this HEAD; failures carry evidence pointers
  scope    --pr N [--base REF]   the top-level directories and file set this PR touches
  history  --pr N --job NAME     that job's conclusion across this branch's HEADs (judges flaky / pre-existing failures)

Review comments are **not here**: they belong to `address-review-comments --github`, which turns comments into a structured
review lane (round identity = the commit the comments pin to); this skill only handles the CI-side signals.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys


class SignalError(RuntimeError):
    """Signals unavailable (gh unavailable, PR does not exist, fields missing)."""


def _gh(*args: str) -> str:
    result = subprocess.run(["gh", *args], capture_output=True, text=True, check=False)
    if result.returncode != 0:
        raise SignalError(f"gh {' '.join(args)} failed: {result.stderr.strip()}")
    return result.stdout


def _git(*args: str) -> str:
    result = subprocess.run(["git", *args], capture_output=True, text=True, check=False)
    if result.returncode != 0:
        raise SignalError(f"git {' '.join(args)} failed: {result.stderr.strip()}")
    return result.stdout


DETAILS_URL_RE = re.compile(r"/actions/runs/(\d+)(?:/job/(\d+))?")

# The rollup's conclusion takes more values than success/failure, and **everything except SUCCESS is not green**.
# List the failure classes explicitly; every other non-SUCCESS (CANCELLED / SKIPPED / NEUTRAL / STALE…) goes into `other`
# for the agent to name and handle — never treat it as passing just because it is "not on the failure list".
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
        raise SignalError(f"PR {pr} has no headRefOid")
    return data


def _normalize_check(entry: dict) -> dict:
    """Unify the rollup's two kinds of entries into one shape.

    Actions entries are `CheckRun` (with name / workflowName / status / conclusion);
    external statuses are `StatusContext` (only context / state, no status dimension).
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
    """Every check conclusion for this HEAD, per **check** (not per workflow).

    The source of truth is `statusCheckRollup`, the one GitHub itself uses to decide whether a PR can merge. Do not switch back to
    `gh run list`: it is workflow-granular, and in this repo the `CI` workflow carries `conventional-commits` /
    `check (ubuntu-latest)` / `check (macos-latest)` / `check (windows-latest)` / `Windows ConPTY package` as independent
    checks; collapsed into one name, the "which platform's which job went red" that attribution rests on is gone; it also lists
    workflows such as `Copilot` that take no part in gating. Both directions are wrong, one under-reports and one over-reports.
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


def scope(pr: str, base: str | None) -> dict:
    """What this PR actually touches. The first-hand basis for attribution."""

    info = pr_view(pr)
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
    """The same check's conclusion across this branch's HEADs.

    `job` is the **per-check name** from `checks` (such as `check (windows-latest)`), not the workflow name.
    History must be taken per check too: workflow-granular history mixes the outcomes of several checks under one workflow into one
    conclusion, so "this one was red before" and "I turned this one red" can no longer be told apart — and that is this command's only purpose.
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


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    c = sub.add_parser("checks", help="every check conclusion for this HEAD")
    c.add_argument("--pr", required=True)

    s = sub.add_parser("scope", help="the files and top-level directories this PR touches")
    s.add_argument("--pr", required=True)
    s.add_argument("--base", default=None)

    h = sub.add_parser("history", help="one check's conclusion across this branch's HEADs")
    h.add_argument("--pr", required=True)
    h.add_argument("--job", required=True, help="the per-check name from checks, not the workflow name")
    h.add_argument("--limit", type=int, default=12, help="how many workflow runs to look back at most (default 12)")

    args = parser.parse_args(argv)
    try:
        if args.command == "checks":
            payload = checks(args.pr)
        elif args.command == "scope":
            payload = scope(args.pr, args.base)
        else:
            payload = history(args.pr, args.job, args.limit)
        print(json.dumps(payload, ensure_ascii=False, indent=2))
        return 0
    except SignalError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
