"""pr_signals 的机械契约：事实源、轮次切分、以及 fail-closed。

这里钉的几件事都直接对应 guide 里记的实测教训：
  - check 结论必须来自 `statusCheckRollup`（GitHub 用来 gating 的那份），逐**条**而非逐 workflow；
  - 非 SUCCESS 且不在失败名单里的结论不得被当成通过；
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest


SCRIPTS_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS_DIR))

import pr_signals  # noqa: E402
from pr_signals import SignalError  # noqa: E402


HEAD = "a" * 40


def _check(name, status, conclusion=None, workflow="Unit Test", run=1, job=2):
    return {
        "__typename": "CheckRun",
        "name": name,
        "workflowName": workflow,
        "status": status,
        "conclusion": conclusion,
        "detailsUrl": f"https://x.invalid/actions/runs/{run}/job/{job}",
    }


def _pr(rollup, head=HEAD):
    return {
        "number": 791,
        "headRefName": "feat/x",
        "headRefOid": head,
        "baseRefName": "main",
        "state": "OPEN",
        "isDraft": False,
        "mergeable": "MERGEABLE",
        "mergeStateStatus": "CLEAN",
        "url": "https://example.invalid/791",
        "statusCheckRollup": rollup,
    }


def _fake_gh(monkeypatch, pr=None, runs=None, comments=None, jobs=None):
    def gh(*args):
        if args[0] == "pr":
            return json.dumps(pr if pr is not None else _pr([]))
        if args[0] == "run":
            return json.dumps(runs or [])
        if args[0] == "api" and "/jobs" in args[1]:
            return json.dumps({"jobs": jobs or []})
        if args[0] == "api":
            return json.dumps(comments or [])
        raise AssertionError(f"unexpected gh call: {args}")

    monkeypatch.setattr(pr_signals, "_gh", gh)


# ---------- checks：事实源与分类 ----------


def test_checks_reports_每条_check_而不是_workflow(monkeypatch) -> None:
    """一个 workflow 挂着多条独立 check（本仓如 `Lint Check` 下的 `Glossary Lint` / `RN Unit Test` / `TS Lint`）。

    塌缩成 workflow 名之后，「哪个平台的哪个 job 红了」就没了——而归因的第一条证据
    （范围不相交）正是靠它成立的。
    """
    _fake_gh(
        monkeypatch,
        pr=_pr(
            [
                _check("CLI Unit Test", "COMPLETED", "SUCCESS"),
                _check("Windows CLI Unit Test", "COMPLETED", "SUCCESS"),
                _check("Integration Test", "COMPLETED", "SUCCESS"),
            ]
        ),
    )

    result = pr_signals.checks("791")

    assert result["checkCount"] == 3
    assert result["passed"] == [
        "CLI Unit Test",
        "Integration Test",
        "Windows CLI Unit Test",
    ]


def test_failed_check_carries_job_scoped_log_pointer(monkeypatch) -> None:
    _fake_gh(
        monkeypatch,
        pr=_pr(
            [
                _check("Windows Unit Test", "COMPLETED", "FAILURE", run=77, job=99),
            ]
        ),
    )

    failed = pr_signals.checks("791")["failed"]

    assert failed[0]["name"] == "Windows Unit Test"
    assert failed[0]["workflow"] == "Unit Test"
    assert failed[0]["runId"] == 77 and failed[0]["jobId"] == 99
    assert failed[0]["logCommand"] == "gh run view --job 99 --log-failed"


@pytest.mark.parametrize("conclusion", ["TIMED_OUT", "STARTUP_FAILURE", "ACTION_REQUIRED"])
def test_failure_shaped_conclusions_count_as_failed(monkeypatch, conclusion) -> None:
    _fake_gh(monkeypatch, pr=_pr([_check("Build", "COMPLETED", conclusion)]))

    result = pr_signals.checks("791")

    assert [f["name"] for f in result["failed"]] == ["Build"]
    assert result["passed"] == []


@pytest.mark.parametrize("conclusion", ["CANCELLED", "SKIPPED", "NEUTRAL", "STALE"])
def test_non_success_conclusions_are_never_silently_green(monkeypatch, conclusion) -> None:
    """只有 SUCCESS 是绿。其余的必须留在 `other` 里等着被点名，不能悄悄算通过。"""
    _fake_gh(monkeypatch, pr=_pr([_check("Build", "COMPLETED", conclusion)]))

    result = pr_signals.checks("791")

    assert result["passed"] == []
    assert result["other"] == [{"name": "Build", "conclusion": conclusion}]


def test_incomplete_check_is_pending_not_passed(monkeypatch) -> None:
    _fake_gh(monkeypatch, pr=_pr([_check("E2E Test", "IN_PROGRESS")]))

    result = pr_signals.checks("791")

    assert result["pending"] == ["E2E Test"]
    assert result["passed"] == [] and result["failed"] == []


def test_external_status_contexts_are_included(monkeypatch) -> None:
    """非 Actions 的状态同样参与 gating，不能因为形状不同就漏掉。"""
    _fake_gh(
        monkeypatch,
        pr=_pr(
            [
                {
                    "__typename": "StatusContext",
                    "context": "codecov/patch",
                    "state": "FAILURE",
                    "targetUrl": "https://x.invalid/cov",
                },
                {"__typename": "StatusContext", "context": "codecov/project", "state": "PENDING", "targetUrl": None},
            ]
        ),
    )

    result = pr_signals.checks("791")

    assert [f["name"] for f in result["failed"]] == ["codecov/patch"]
    assert result["pending"] == ["codecov/project"]


def test_empty_rollup_is_not_reported_as_green(monkeypatch) -> None:
    """一条 check 都没有时 checkCount 必须为 0，让调用方看得出「没有信号」。"""
    _fake_gh(monkeypatch, pr=_pr([]))

    result = pr_signals.checks("791")

    assert result["checkCount"] == 0
    assert result["passed"] == [] and result["pending"] == []


# ---------- history：逐条取，不混 workflow ----------


def test_history_matches_the_named_job_across_commits(monkeypatch) -> None:
    _fake_gh(
        monkeypatch,
        pr=_pr([_check("Windows CLI Unit Test", "COMPLETED", "FAILURE")]),
        runs=[
            {
                "databaseId": 2,
                "headSha": HEAD,
                "workflowName": "Unit Test",
                "status": "completed",
                "conclusion": "failure",
                "createdAt": "2026-09-16T02:00:00Z",
            },
            {
                "databaseId": 1,
                "headSha": "b" * 40,
                "workflowName": "Unit Test",
                "status": "completed",
                "conclusion": "success",
                "createdAt": "2026-09-16T01:00:00Z",
            },
            {
                "databaseId": 9,
                "headSha": HEAD,
                "workflowName": "Lint Check",
                "status": "completed",
                "conclusion": "success",
                "createdAt": "2026-09-16T02:30:00Z",
            },
        ],
        jobs=[
            {"name": "Windows CLI Unit Test", "id": 5, "conclusion": "failure"},
            {"name": "CLI Unit Test", "id": 6, "conclusion": "success"},
        ],
    )

    result = pr_signals.history("791", "Windows CLI Unit Test")

    assert result["workflow"] == "Unit Test"
    # Lint Check 那次 run 不属于这条 check 的 owning workflow，必须被排除。
    assert result["runsInspected"] == 2
    assert [r["sha"] for r in result["runs"]] == ["bbbbbbbb", "aaaaaaaa"]
    assert [r["isCurrentHead"] for r in result["runs"]] == [False, True]


# ---------- fail-closed ----------


def test_gh_failure_is_reported_not_swallowed(monkeypatch) -> None:
    """拿不到信号时必须失败，不能返回一个看起来干净的空结果。"""

    def boom(*args):
        raise SignalError("gh 不可用")

    monkeypatch.setattr(pr_signals, "_gh", boom)

    with pytest.raises(SignalError):
        pr_signals.checks("791")


def test_missing_head_oid_fails_closed(monkeypatch) -> None:
    monkeypatch.setattr(pr_signals, "_gh", lambda *a: json.dumps({"number": 1}))

    with pytest.raises(SignalError):
        pr_signals.pr_view("791")


class _FakeCodeup:
    name = "codeup"

    def checks(self, pr):
        return {"pr": int(pr), "head": HEAD, "checkCount": 0}

    def view(self, pr):
        return {"number": int(pr), "headRefOid": HEAD, "baseRefName": "main"}


def test_codeup_checks_come_from_pr_host(monkeypatch, capsys):
    monkeypatch.setattr(pr_signals.pr_host, "detect", lambda: _FakeCodeup())
    monkeypatch.setattr(pr_signals, "_gh", lambda *a: pytest.fail("Codeup 不应调用 gh"))
    assert pr_signals.main(["checks", "--pr", "4"]) == 0
    assert json.loads(capsys.readouterr().out) == {"pr": 4, "head": HEAD, "checkCount": 0}


def test_codeup_scope_uses_pr_host_view(monkeypatch, capsys):
    monkeypatch.setattr(pr_signals.pr_host, "detect", lambda: _FakeCodeup())
    monkeypatch.setattr(pr_signals, "_git", lambda *a: "m" * 40 if a[0] == "merge-base" else "a/x.py\nb.md\n")
    assert pr_signals.main(["scope", "--pr", "4"]) == 0
    payload = json.loads(capsys.readouterr().out)
    assert (payload["base"], payload["topLevel"]) == ("origin/main", ["a", "b.md"])


def test_codeup_history_fails_closed(monkeypatch, capsys):
    monkeypatch.setattr(pr_signals.pr_host, "detect", lambda: _FakeCodeup())
    assert pr_signals.main(["history", "--pr", "4", "--job", "lint"]) == 2
    assert "不提供逐条 check 的历史结论" in capsys.readouterr().err
