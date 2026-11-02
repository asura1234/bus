"""Repeated Bus snapshots must not double delivery failures or compactions."""

import json
import subprocess
import sys
from pathlib import Path

CLI = Path(__file__).resolve().parents[1] / "agent_stats.py"


def call(*args):
    return subprocess.run(
        [sys.executable, str(CLI), *map(str, args)], text=True, capture_output=True
    )


def test_observe_retains_delivery_reasons_and_compaction_counts(tmp_path):
    state = {
        "ok": True,
        "result": {
            "rooms": [{"id": 8, "orchestrator": 3}],
            "agents": [
                {
                    "id": 2,
                    "name": "worker",
                    "provider": "codex",
                    "room_id": 8,
                    "compactions": {"count": 3, "last_at_ms": 1000},
                },
                {"id": 3, "name": "orch", "provider": "claude_code", "orchestrates": 8},
            ],
        },
    }
    sp = tmp_path / "state.json"
    sp.write_text(json.dumps(state))
    store = tmp_path / "stats"
    assigned = call(
        "assign",
        "--store",
        store,
        "--task",
        "x",
        "--role",
        "author",
        "--agent",
        "2",
        "--orchestrator",
        "3",
        "--state",
        sp,
        "--message",
        "81",
    )
    assert assigned.returncode == 0, assigned.stderr
    status = {
        "ok": True,
        "result": {
            "message_id": 81,
            "requests": [
                {
                    "request_id": 82,
                    "agent_id": 2,
                    "stage": "abandoned",
                    "reason": "reply_not_captured",
                }
            ],
        },
    }
    mp = tmp_path / "status.json"
    mp.write_text(json.dumps(status))
    for _ in range(2):
        r = call("observe", "--store", store, "--state", sp, "--status", mp)
        assert r.returncode == 0, r.stderr
    result = json.loads(call("report", "--store", store, "--json").stdout)
    assert result["leaderboard"][0]["abandoned_deliveries"] == 1
    assert result["compactions_by_agent"]["worker"] == 3
    rows = [json.loads(x) for x in (store / "events.jsonl").read_text().splitlines()]
    assert any(
        r["kind"] == "delivery" and r["data"]["reason"] == "reply_not_captured"
        for r in rows
    )
