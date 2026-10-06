"""The ledger's mechanical contract: count only originating findings, append per line, adjudication counts must be passed explicitly."""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest


sys.path.insert(0, str(Path(__file__).resolve().parent))

import review_metrics as rm  # noqa: E402


def _stub(monkeypatch, reviews, comments, info=None):
    def gh_json(*args, **kwargs):
        if args[0] == "pr":
            return info or {
                "baseRefName": "master",
                "headRefOid": "a" * 40,
                "additions": 10,
                "deletions": 2,
                "changedFiles": 3,
                "mergedAt": "2026-09-20T06:00:00Z",
            }
        return reviews if "/reviews" in args[1] else comments

    monkeypatch.setattr(rm, "_gh_json", gh_json)


def test_replies_are_not_counted_as_findings(monkeypatch) -> None:
    """Replies hang off `in_reply_to_id`. Counting them in would inflate the finding count with our own replies,
    and that is exactly the quantity this ledger uses to judge how fast things converge."""

    _stub(
        monkeypatch,
        reviews=[{"body": "### 🟡 Changes recommended"}, {"body": ""}],
        comments=[{"id": 1, "in_reply_to_id": None}, {"id": 2, "in_reply_to_id": None}, {"id": 3, "in_reply_to_id": 1}],
    )

    record = rm.collect("899", Path("."))

    assert record["inlineFindings"] == 2
    assert record["repliedFindings"] == 1
    assert record["reviewPasses"] == 1, "an empty-body COMMENTED does not count as a review pass"


def test_state_only_reviews_count_as_a_pass(monkeypatch) -> None:
    """When a human only clicks Request changes / Approve there is no body, but it really is a review pass."""

    _stub(
        monkeypatch,
        reviews=[{"body": "", "state": "CHANGES_REQUESTED"}, {"body": "", "state": "COMMENTED"}],
        comments=[],
    )

    assert rm.collect("899", Path("."))["reviewPasses"] == 1


def test_body_only_findings_are_counted_separately(monkeypatch) -> None:
    """A body-only review has no inline comments; counting only inline would record the whole round as "0 findings"."""

    _stub(monkeypatch, reviews=[{"body": "### 🟡 Changes recommended\n\nOpen (3)", "state": "COMMENTED"}], comments=[])

    record = rm.collect("899", Path("."))
    assert record["inlineFindings"] == 0
    assert record["reviewsWithBodyFindings"] == 1


def test_each_merge_appends_one_line(tmp_path) -> None:
    ledger = tmp_path / "ci" / "review-metrics.jsonl"
    rm.append({"pr": 1}, ledger)
    rm.append({"pr": 2}, ledger)

    lines = ledger.read_text(encoding="utf-8").strip().splitlines()
    assert [json.loads(x)["pr"] for x in lines] == [1, 2]


def test_gh_runs_in_the_selected_repository(monkeypatch, tmp_path) -> None:
    """Without cwd, `gh` resolves `{owner}/{repo}` from the caller's current directory.

    So running `--repo <path>` from another checkout queries repository A's PR and writes the record into repository B's ledger —
    a line describing the wrong PR's data, while this ledger's entire purpose is long-term trends.
    """

    seen: list = []

    def fake_run(argv, **kwargs):
        seen.append(kwargs.get("cwd"))

        class R:
            returncode = 0
            stdout = "[]" if argv[1] == "api" else '{"changedFiles": 1}'
            stderr = ""

        return R()

    monkeypatch.setattr(rm.subprocess, "run", fake_run)
    rm.collect("899", tmp_path)

    assert seen and all(c == str(tmp_path) for c in seen), seen


def test_adjudication_counts_must_be_supplied() -> None:
    """APPLY / REJECT have no source of truth on GitHub and can only be passed in by the main agent; when missing, exit instead of guessing 0."""

    with pytest.raises(SystemExit):
        rm.main(["--pr", "899"])
