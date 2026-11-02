"""Recorded PR #295 proves canonical root credit without semantic deduplication."""

import json
import subprocess
import sys
from pathlib import Path

CLI = Path(__file__).resolve().parents[1] / "agent_stats.py"
FIXTURE = Path(__file__).with_name("fixtures") / "pr295"


def call(*args):
    return subprocess.run(
        [sys.executable, str(CLI), *map(str, args)], text=True, capture_output=True
    )


def test_recorded_reviews_use_triage_root_credits_and_preserve_unknowns(tmp_path):
    args = (
        "collect",
        "--store",
        tmp_path / "stats",
        "--task",
        "pr-295",
        "--artifacts",
        FIXTURE,
    )
    result = call(*args)
    assert result.returncode == 0, result.stderr
    assert call(*args).returncode == 0
    rows = [
        json.loads(x)
        for x in (tmp_path / "stats/events.jsonl").read_text().splitlines()
    ]
    reviews = [x for x in rows if x["kind"] == "review"]
    assert len(reviews) == 4
    roots = [x for x in rows if x["kind"] == "root_cause"]
    assert len(roots) == 2
    assert [(r["data"]["outcome"], r["data"]["reviewers"]) for r in roots] == [
        ("APPLY", ["clauderev5"]),
        ("APPLY", ["clauderev5", "codexrevf"]),
    ]
    assert all(x["agent"]["model"] == "unknown" for x in rows)
    assert call("validate", tmp_path / "stats/events.jsonl").returncode == 0
    result = call("report", "--store", tmp_path / "stats", "--json")
    assert result.returncode == 0, result.stderr
    report = json.loads(result.stdout)
    credits = {x["agent"]: x for x in report["leaderboard"]}
    assert credits["clauderev5"]["unique_issues"] == 1
    assert credits["clauderev5"]["shared_issues"] == 1
    assert credits["codexrevf"]["unique_issues"] == 0
    assert credits["codexrevf"]["shared_issues"] == 1
    assert credits["clauderev5"]["precision"] == 1.0
    assert report["bottlenecks"]["unmeasured"]


def test_gate_deduplicates_copies_and_does_not_guess_failure_cause(tmp_path):
    root = tmp_path / "artifacts"
    root.mkdir()
    gate = {
        "head": "a" * 40,
        "exit_code": 1,
        "duration_ms": 12500,
        "coverage_ms": None,
        "selection": {"full": True},
        "results": [
            {
                "name": "python",
                "exit_code": 1,
                "duration_ms": 11000,
                "stdout": "FAILED tests/test_x.py::test_y - bad",
                "stderr": "",
            }
        ],
    }
    (root / "gate.json").write_text(json.dumps(gate))
    (root / "copy.json").write_text(json.dumps(gate))
    result = call(
        "collect", "--store", tmp_path / "stats", "--task", "pr-7", "--artifacts", root
    )
    assert result.returncode == 0, result.stderr
    rows = [
        json.loads(x)
        for x in (tmp_path / "stats/events.jsonl").read_text().splitlines()
    ]
    runs = [r for r in rows if r["kind"] == "run"]
    assert len(runs) == 1
    assert runs[0]["data"]["result"] == "FAIL"
    assert runs[0]["data"]["duration_ms"] == 12500
    assert runs[0]["data"]["failure_cause"] is None


def test_conflicting_review_copies_fail_without_partial_import(tmp_path):
    root = tmp_path / "artifacts"
    root.mkdir()
    original = (FIXTURE / "clauderev5/round-02/review.md").read_text()
    (root / "review.md").write_text(original)
    (root / "second.md").write_text(
        original.replace("**判定**：Ready", "**判定**：Abandon")
    )
    result = call(
        "collect",
        "--store",
        tmp_path / "stats",
        "--task",
        "pr-295",
        "--artifacts",
        root,
    )
    assert result.returncode == 1
    assert not (tmp_path / "stats/events.jsonl").exists()


def test_unknown_structured_source_lane_fails_closed(tmp_path):
    root = tmp_path / "artifacts"
    root.mkdir()
    (root / "triage.md").write_text(
        "**模式**：pr\n\n## APPLY\n\n- C1 — root\n\n## Root cause provenance\n\n| claim_id | outcome | sources |\n|---|---|---|\n| C1 | APPLY | ghost@1:1 |\n"
    )
    result = call(
        "collect", "--store", tmp_path / "stats", "--task", "pr-1", "--artifacts", root
    )
    assert result.returncode == 1
    assert not (tmp_path / "stats/events.jsonl").exists()
