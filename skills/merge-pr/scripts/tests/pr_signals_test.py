"""pr_signals' mechanical contract: source of truth, round slicing, and fail-closed.

The few things pinned here each map directly to a measured lesson recorded in the guide:
  - check conclusions must come from `statusCheckRollup` (the one GitHub gates on), per **check** not per workflow;
  - conclusions that are not SUCCESS and not on the failure list must never be treated as passing;
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


def _check(name, status, conclusion=None, workflow="CI", run=1, job=2):
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
        "baseRefName": "master",
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


# ---------- checks: source of truth and classification ----------


def test_checks_reports_each_check_not_the_workflow(monkeypatch) -> None:
    """One workflow carries several independent checks (in this repo, `CI` carries `check (ubuntu-latest)` / `check (macos-latest)` / `check (windows-latest)`).

    Collapsed into the workflow name, "which platform's which job went red" is gone — and the first kind of attribution evidence
    (disjoint scope) rests on exactly that.
    """
    _fake_gh(
        monkeypatch,
        pr=_pr(
            [
                _check("check (ubuntu-latest)", "COMPLETED", "SUCCESS"),
                _check("check (windows-latest)", "COMPLETED", "SUCCESS"),
                _check("check (macos-latest)", "COMPLETED", "SUCCESS"),
            ]
        ),
    )

    result = pr_signals.checks("791")

    assert result["checkCount"] == 3
    assert result["passed"] == [
        "check (macos-latest)",
        "check (ubuntu-latest)",
        "check (windows-latest)",
    ]


def test_failed_check_carries_job_scoped_log_pointer(monkeypatch) -> None:
    _fake_gh(
        monkeypatch,
        pr=_pr(
            [
                _check("check (windows-latest)", "COMPLETED", "FAILURE", run=77, job=99),
            ]
        ),
    )

    failed = pr_signals.checks("791")["failed"]

    assert failed[0]["name"] == "check (windows-latest)"
    assert failed[0]["workflow"] == "CI"
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
    """Only SUCCESS is green. Everything else must stay in `other` waiting to be named, never quietly counted as passing."""
    _fake_gh(monkeypatch, pr=_pr([_check("Build", "COMPLETED", conclusion)]))

    result = pr_signals.checks("791")

    assert result["passed"] == []
    assert result["other"] == [{"name": "Build", "conclusion": conclusion}]


def test_incomplete_check_is_pending_not_passed(monkeypatch) -> None:
    _fake_gh(monkeypatch, pr=_pr([_check("Windows ConPTY package", "IN_PROGRESS")]))

    result = pr_signals.checks("791")

    assert result["pending"] == ["Windows ConPTY package"]
    assert result["passed"] == [] and result["failed"] == []


def test_external_status_contexts_are_included(monkeypatch) -> None:
    """Non-Actions statuses take part in gating too; they must not be missed just because their shape differs."""
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
    """With zero checks, checkCount must be 0 so the caller can see "no signal"."""
    _fake_gh(monkeypatch, pr=_pr([]))

    result = pr_signals.checks("791")

    assert result["checkCount"] == 0
    assert result["passed"] == [] and result["pending"] == []


# ---------- history: taken per check, never mixing workflows ----------


def test_history_matches_the_named_job_across_commits(monkeypatch) -> None:
    _fake_gh(
        monkeypatch,
        pr=_pr([_check("check (windows-latest)", "COMPLETED", "FAILURE")]),
        runs=[
            {
                "databaseId": 2,
                "headSha": HEAD,
                "workflowName": "CI",
                "status": "completed",
                "conclusion": "failure",
                "createdAt": "2026-09-16T02:00:00Z",
            },
            {
                "databaseId": 1,
                "headSha": "b" * 40,
                "workflowName": "CI",
                "status": "completed",
                "conclusion": "success",
                "createdAt": "2026-09-16T01:00:00Z",
            },
            {
                "databaseId": 9,
                "headSha": HEAD,
                "workflowName": "Nix",
                "status": "completed",
                "conclusion": "success",
                "createdAt": "2026-09-16T02:30:00Z",
            },
        ],
        jobs=[
            {"name": "check (windows-latest)", "id": 5, "conclusion": "failure"},
            {"name": "check (ubuntu-latest)", "id": 6, "conclusion": "success"},
        ],
    )

    result = pr_signals.history("791", "check (windows-latest)")

    assert result["workflow"] == "CI"
    # The Nix run does not belong to this check's owning workflow and must be excluded.
    assert result["runsInspected"] == 2
    assert [r["sha"] for r in result["runs"]] == ["bbbbbbbb", "aaaaaaaa"]
    assert [r["isCurrentHead"] for r in result["runs"]] == [False, True]


# ---------- fail-closed ----------


def test_gh_failure_is_reported_not_swallowed(monkeypatch) -> None:
    """When signals are unavailable it must fail, never return a clean-looking empty result."""

    def boom(*args):
        raise SignalError("gh unavailable")

    monkeypatch.setattr(pr_signals, "_gh", boom)

    with pytest.raises(SignalError):
        pr_signals.checks("791")


def test_missing_head_oid_fails_closed(monkeypatch) -> None:
    monkeypatch.setattr(pr_signals, "_gh", lambda *a: json.dumps({"number": 1}))

    with pytest.raises(SignalError):
        pr_signals.pr_view("791")
