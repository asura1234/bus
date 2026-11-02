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
