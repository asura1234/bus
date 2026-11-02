"""Task boundaries derive elapsed time; callers never type a duration."""

import json
import subprocess
import sys
from pathlib import Path

CLI = Path(__file__).resolve().parents[1] / "agent_stats.py"


def call(*args):
    return subprocess.run(
        [sys.executable, str(CLI), *map(str, args)], capture_output=True, text=True
    )


def test_phase_boundaries_pair_and_reject_duplicate_or_unassigned_work(tmp_path):
    state = tmp_path / "state.json"
    state.write_text(
        json.dumps(
            {
                "ok": True,
                "result": {
                    "rooms": [{"id": 8, "orchestrator": 3}],
                    "agents": [
                        {"id": 2, "name": "author", "provider": "codex", "room_id": 8},
                        {
                            "id": 3,
                            "name": "orch",
                            "provider": "claude_code",
                            "orchestrates": 8,
                        },
                    ],
                },
            }
        )
    )
    common = ("--store", tmp_path / "stats", "--task", "pr-12", "--agent", "2")
    assert call("task-start", *common, "--phase", "work").returncode == 1
    assert (
        call(
            "assign",
            *common,
            "--role",
            "author",
            "--orchestrator",
            "3",
            "--state",
            state,
        ).returncode
        == 0
    )
    start = call("task-start", *common, "--phase", "work")
    assert start.returncode == 0, start.stderr
    assert call("task-start", *common, "--phase", "in_review").returncode == 1
    assert call("task-end", *common, "--phase", "in_review").returncode == 1
    end = call("task-end", *common, "--phase", "work")
    assert end.returncode == 0, end.stderr
    assert call("task-end", *common, "--phase", "work").returncode == 1
    rows = [
        json.loads(x)
        for x in (tmp_path / "stats/events.jsonl").read_text().splitlines()
    ]
    phases = [r for r in rows if r["kind"] == "phase"]
    assert len(phases) == 2
    assert phases[1]["data"]["start_id"] == phases[0]["id"]
    assert phases[1]["data"]["duration_ms"] >= 0
    assert phases[1]["agent"]["name"] == "author"
    report = call("report", "--store", tmp_path / "stats", "--json")
    assert json.loads(report.stdout)["bottlenecks"]["measured"]["phase_work_ms"] >= 0
    assert call("task-start", *common, "--phase", "waiting_build_slot").returncode == 0
    report = call("report", "--store", tmp_path / "stats", "--json")
    assert json.loads(report.stdout)["bottlenecks"]["open_phases"] == 1


def test_phase_end_binds_exact_review_artifact_to_observed_assignment(tmp_path):
    fixture = (
        Path(__file__).with_name("fixtures") / "pr295/clauderev5/round-02/review.md"
    )
    state = tmp_path / "state.json"
    state.write_text(
        json.dumps(
            {
                "ok": True,
                "result": {
                    "rooms": [{"id": 8, "orchestrator": 3}],
                    "agents": [
                        {
                            "id": 2,
                            "name": "clauderev5",
                            "provider": "claude_code",
                            "room_id": 8,
                        },
                        {
                            "id": 3,
                            "name": "orch",
                            "provider": "claude_code",
                            "orchestrates": 8,
                        },
                    ],
                },
            }
        )
    )
    common = ("--store", tmp_path / "stats", "--task", "pr-295", "--agent", "2")
    assert (
        call(
            "assign",
            *common,
            "--role",
            "reviewer",
            "--orchestrator",
            "3",
            "--state",
            state,
        ).returncode
        == 0
    )
    assert call("task-start", *common, "--phase", "in_review").returncode == 0
    end = call("task-end", *common, "--phase", "in_review", "--artifact", fixture)
    assert end.returncode == 0, end.stderr
    assert Path(json.loads(end.stdout)["receipt"]).is_file()
    root = tmp_path / "artifacts"
    root.mkdir()
    (root / "review.md").write_bytes(fixture.read_bytes())
    result = call(
        "collect",
        "--store",
        tmp_path / "stats",
        "--task",
        "pr-295",
        "--artifacts",
        root,
    )
    assert result.returncode == 0, result.stderr
    rows = [
        json.loads(x)
        for x in (tmp_path / "stats/events.jsonl").read_text().splitlines()
    ]
    review = [r for r in rows if r["kind"] == "review"][0]
    assert review["agent"]["id"] == "2"
    assert review["agent"]["provider"] == "claude_code"
    assert review["ts"] is not None


def test_validation_artifact_binds_tester_identity_to_its_run(tmp_path):
    state = tmp_path / "state.json"
    state.write_text(
        json.dumps(
            {
                "ok": True,
                "result": {
                    "rooms": [{"id": 8, "orchestrator": 3}],
                    "agents": [
                        {"id": 2, "name": "tester", "provider": "codex", "room_id": 8},
                        {
                            "id": 3,
                            "name": "orch",
                            "provider": "claude_code",
                            "orchestrates": 8,
                        },
                    ],
                },
            }
        )
    )
    root = tmp_path / "artifacts"
    root.mkdir()
    artifact = root / "run.json"
    artifact.write_text(
        json.dumps(
            {
                "head": "a" * 40,
                "exit_code": 0,
                "duration_ms": 100,
                "coverage_ms": None,
                "selection": {"full": True},
                "results": [],
            }
        )
    )
    common = ("--store", tmp_path / "stats", "--task", "pr-12", "--agent", "2")
    assert (
        call(
            "assign",
            *common,
            "--role",
            "tester",
            "--orchestrator",
            "3",
            "--state",
            state,
        ).returncode
        == 0
    )
    assert call("task-start", *common, "--phase", "gate").returncode == 0
    assert (
        call("task-end", *common, "--phase", "gate", "--artifact", artifact).returncode
        == 0
    )
    r = call(
        "collect", "--store", tmp_path / "stats", "--task", "pr-12", "--artifacts", root
    )
    assert r.returncode == 0, r.stderr
    rows = [
        json.loads(x)
        for x in (tmp_path / "stats/events.jsonl").read_text().splitlines()
    ]
    run = [r for r in rows if r["kind"] == "run"][0]
    assert run["agent"]["id"] == "2"
    assert run["agent"]["provider"] == "codex"


def test_phase_end_preserves_artifact_evidence_before_worktree_cleanup(tmp_path):
    import hashlib

    state = tmp_path / "state.json"
    state.write_text(
        json.dumps(
            {
                "ok": True,
                "result": {
                    "rooms": [{"id": 8, "orchestrator": 3}],
                    "agents": [
                        {"id": 2, "name": "worker", "provider": "codex", "room_id": 8},
                        {
                            "id": 3,
                            "name": "orch",
                            "provider": "claude_code",
                            "orchestrates": 8,
                        },
                    ],
                },
            }
        )
    )
    artifact = tmp_path / "review.md"
    artifact.write_text("recorded result")
    common = ("--store", tmp_path / "stats", "--task", "x", "--agent", "2")
    assert (
        call(
            "assign",
            *common,
            "--role",
            "reviewer",
            "--orchestrator",
            "3",
            "--state",
            state,
        ).returncode
        == 0
    )
    assert call("task-start", *common, "--phase", "in_review").returncode == 0
    end = call("task-end", *common, "--phase", "in_review", "--artifact", artifact)
    assert end.returncode == 0, end.stderr
    artifact.unlink()
    event = json.loads(Path(json.loads(end.stdout)["receipt"]).read_text())
    for src in event["sources"]:
        assert Path(src["path"]).exists()
        assert (
            hashlib.sha256(Path(src["path"]).read_bytes()).hexdigest() == src["sha256"]
        )


def test_modified_start_clock_cannot_create_an_elapsed_duration(tmp_path):
    state = tmp_path / "state.json"
    state.write_text(
        json.dumps(
            {
                "ok": True,
                "result": {
                    "rooms": [{"id": 8, "orchestrator": 3}],
                    "agents": [
                        {"id": 2, "name": "worker", "provider": "codex", "room_id": 8},
                        {
                            "id": 3,
                            "name": "orch",
                            "provider": "claude_code",
                            "orchestrates": 8,
                        },
                    ],
                },
            }
        )
    )
    common = ("--store", tmp_path / "stats", "--task", "x", "--agent", "2")
    assert (
        call(
            "assign",
            *common,
            "--role",
            "author",
            "--orchestrator",
            "3",
            "--state",
            state,
        ).returncode
        == 0
    )
    start = call("task-start", *common, "--phase", "work")
    assert start.returncode == 0, start.stderr
    row = json.loads(Path(json.loads(start.stdout)["receipt"]).read_text())
    path = Path(row["sources"][0]["path"])
    clock = json.loads(path.read_text())
    clock["monotonic_ns"] -= 100000000000
    path.write_text(json.dumps(clock))
    end = call("task-end", *common, "--phase", "work")
    assert end.returncode == 1
    assert "clock" in end.stderr
