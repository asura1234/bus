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


def test_archived_triage_copy_with_extra_validation_does_not_double_root_credit(
    tmp_path,
):
    import shutil

    root = tmp_path / "artifacts"
    shutil.copytree(FIXTURE, root)
    copy = root / "evidence/author-triage.md"
    copy.parent.mkdir()
    copy.write_text(
        (root / "author/triage.md").read_text()
        + "\n## Later validation\n\nAll probes pass.\n"
    )
    r = call(
        "collect",
        "--store",
        tmp_path / "stats",
        "--task",
        "pr-295",
        "--artifacts",
        root,
    )
    assert r.returncode == 0, r.stderr
    rows = [
        json.loads(x)
        for x in (tmp_path / "stats/events.jsonl").read_text().splitlines()
    ]
    assert len([r for r in rows if r["kind"] == "root_cause"]) == 2


def test_archived_gate_and_e2e_formats_preserve_results_without_inventing_elapsed(
    tmp_path,
):
    root = tmp_path / "artifacts"
    root.mkdir()
    (root / "gate.md").write_text(
        "# Gate-and-Fix Round\n\n- Schema: `2`\n- Round: `2`\n- Base: `306b177332aaefd3addfbbb6c6f1cafb1b3b8a7c`\n- Head: `ad9dc509e1a63a8ecd99cb4136d01e54a1ce9266`\n- Outcome: `PASS`\n\n## Gate results\n\n### lint — PASS\n\n- Exit code: `0`\n- Duration: `124992ms`\n"
    )
    (root / "e2e.md").write_text(
        "# E2E TUI Run\n\n- Commit: `8af9163bdadc22341a998869b30e41f6be82beb1`\n\n## Summary\n\nERRPIPE-01 PASS\nVerdict: GREEN\n"
    )
    (root / "live.json").write_text(
        json.dumps(
            {
                "result": "PASS",
                "head": "ad9dc509e1a63a8ecd99cb4136d01e54a1ce9266",
                "provider": "codex",
                "recovered_request": {"stage": "replied"},
            }
        )
    )
    r = call(
        "collect",
        "--store",
        tmp_path / "stats",
        "--task",
        "pr-295",
        "--artifacts",
        root,
    )
    assert r.returncode == 0, r.stderr
    rows = [
        json.loads(x)
        for x in (tmp_path / "stats/events.jsonl").read_text().splitlines()
    ]
    runs = {r["data"]["kind"]: r["data"] for r in rows if r["kind"] == "run"}
    assert set(runs) == {"gate", "e2e", "live"}
    assert runs["gate"]["lanes"] == [
        {"name": "lint", "result": "PASS", "duration_ms": 124992}
    ]
    assert all(x["duration_ms"] is None for x in runs.values())
