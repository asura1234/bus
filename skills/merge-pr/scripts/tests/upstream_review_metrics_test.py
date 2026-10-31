"""度量台账的机械契约：只数发起的 finding、按行追加、裁决计数必须显式传入。"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest


sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import review_metrics as rm  # noqa: E402


def _stub(monkeypatch, reviews, comments, info=None):
    payload = {
        "info": info or {
            "baseRefName": "main",
            "headRefOid": "a" * 40,
            "additions": 10,
            "deletions": 2,
            "changedFiles": 3,
            "mergedAt": "2026-09-20T06:00:00Z",
        },
        "reviews": reviews,
        "threads": comments,
    }
    monkeypatch.setattr(rm, "_host_metrics", lambda *args, **kwargs: payload)


def test_replies_are_not_counted_as_findings(monkeypatch) -> None:
    """回复挂在 `in_reply_to_id` 上。把它们一起数进去，finding 数会随我们自己的回复膨胀，
    而那恰恰是这份台账要用来判断收敛快慢的那个量。"""

    _stub(
        monkeypatch,
        reviews=[{"body": "### 🟡 Changes recommended"}, {"body": ""}],
        comments=[{"id": 1, "in_reply_to_id": None}, {"id": 2, "in_reply_to_id": None}, {"id": 3, "in_reply_to_id": 1}],
    )

    record = rm.collect("899", Path("."))

    assert record["inlineFindings"] == 2
    assert record["repliedFindings"] == 1
    assert record["reviewPasses"] == 1, "空正文的 COMMENTED 不算一遍评审"


def test_state_only_reviews_count_as_a_pass(monkeypatch) -> None:
    """人类只点 Request changes / Approve 时没有正文，但那确实是一遍评审。"""

    _stub(
        monkeypatch,
        reviews=[{"body": "", "state": "CHANGES_REQUESTED"}, {"body": "", "state": "COMMENTED"}],
        comments=[],
    )

    assert rm.collect("899", Path("."))["reviewPasses"] == 1


def test_body_only_findings_are_counted_separately(monkeypatch) -> None:
    """body-only 评审没有 inline 评论，只数 inline 会把整轮记成「0 条意见」。"""

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


def test_host_metrics_runs_in_the_selected_repository(monkeypatch, tmp_path) -> None:
    """从另一个检出调用时，共享 host 也必须在 `--repo` 指向的仓库里解析 PR。"""

    seen = []

    class Host:
        def metrics(self, pr):
            assert pr == "899"
            seen.append(Path.cwd())
            return {"info": {"changedFiles": 1}, "reviews": [], "threads": []}

    monkeypatch.setattr(rm.pr_host, "detect", lambda: Host())
    rm.collect("899", tmp_path)

    assert seen == [tmp_path]


def test_adjudication_counts_must_be_supplied(monkeypatch) -> None:
    """APPLY / REJECT 在 GitHub 上没有事实源，只能由主 agent 传入；缺了就退出而不是猜 0。"""

    with pytest.raises(SystemExit):
        rm.main(["--pr", "899"])
