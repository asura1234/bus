"""Exercise the published CLI: rejected inputs must never enter the store."""

import json
import subprocess
import sys
from pathlib import Path

CLI = Path(__file__).resolve().parents[1] / "agent_stats.py"


def call(*args):
    return subprocess.run(
        [sys.executable, str(CLI), *map(str, args)], text=True, capture_output=True
    )


def test_validate_rejects_unrecognized_and_untrusted_records(tmp_path):
    source = tmp_path / "input.jsonl"
    for raw in [
        "{}",
        '{"kind":"assignment","kind":"pr"}',
        '{"schema_version":true}',
        '{"kind":"rating","score":10}',
    ]:
        source.write_text(raw + "\n")
        result = call("validate", source)
        assert result.returncode == 1, result.stderr
        assert "invalid" in result.stderr.lower()


def test_assignment_captures_identity_model_and_is_idempotent(tmp_path):
    state = {
        "ok": True,
        "result": {
            "rooms": [{"id": 8, "orchestrator": 3}],
            "agents": [
                {
                    "id": 2,
                    "name": "author",
                    "provider": "codex",
                    "room_id": 8,
                    "runtime_identity": {"launch_id": "launch-a"},
                },
                {
                    "id": 3,
                    "name": "orch",
                    "provider": "claude_code",
                    "orchestrates": 8,
                    "runtime_identity": {"launch_id": "launch-o"},
                },
            ],
        },
    }
    state_path = tmp_path / "state.json"
    state_path.write_text(json.dumps(state))
    pane = {
        "ok": True,
        "result": {
            "agent_id": 2,
            "runtime": {"launch_id": "launch-a", "session_verified": True},
            "text": "Some task mentions Claude Opus 99\n  GPT-6.1-Sol xhigh · ~/work/bus · Task title\n",
        },
    }
    pane_path = tmp_path / "pane.json"
    pane_path.write_text(json.dumps(pane))
    args = (
        "assign",
        "--store",
        tmp_path / "stats",
        "--task",
        "pr-12",
        "--role",
        "author",
        "--agent",
        "2",
        "--orchestrator",
        "3",
        "--state",
        state_path,
        "--pane",
        pane_path,
        "--message",
        "81",
        "--at",
        "2026-11-02T01:00:00Z",
    )
    first = call(*args)
    assert first.returncode == 0, first.stderr
    assert call(*args).returncode == 0
    records = [
        json.loads(x)
        for x in (tmp_path / "stats/events.jsonl").read_text().splitlines()
    ]
    assert len(records) == 1
    rec = records[0]
    assert rec["agent"] == {
        "id": "2",
        "name": "author",
        "provider": "codex",
        "model": "GPT-6.1-Sol xhigh",
    }
    assert rec["orchestrator"]["model"] == "unknown"
    assert rec["data"]["message_id"] == 81
    assert rec["sources"][0]["sha256"]
    assert call("validate", tmp_path / "stats/events.jsonl").returncode == 0
    # A matched agent but different launch cannot lend its model to this assignment.
    pane["result"]["runtime"]["launch_id"] = "other"
    pane_path.write_text(json.dumps(pane))
    assert call(*args).returncode == 1
    assert len((tmp_path / "stats/events.jsonl").read_text().splitlines()) == 1


def test_unknown_role_bad_orchestrator_and_malformed_store_fail_without_write(tmp_path):
    state = tmp_path / "state.json"
    state.write_text(json.dumps({"ok": True, "result": {"rooms": [], "agents": []}}))
    result = call(
        "assign",
        "--store",
        tmp_path / "stats",
        "--task",
        "x",
        "--role",
        "wizard",
        "--agent",
        "2",
        "--orchestrator",
        "3",
        "--state",
        state,
    )
    assert result.returncode != 0
    assert not (tmp_path / "stats/events.jsonl").exists()


def test_valid_record_schema_mutations_and_concurrent_append(tmp_path):
    from concurrent.futures import ThreadPoolExecutor
    import hashlib

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
    args = (
        "assign",
        "--store",
        tmp_path / "stats",
        "--task",
        "pr-12",
        "--role",
        "author",
        "--agent",
        "2",
        "--orchestrator",
        "3",
        "--state",
        state,
        "--message",
        "81",
        "--at",
        "2026-11-02T01:00:00Z",
    )
    results = list(ThreadPoolExecutor(8).map(lambda _: call(*args), range(8)))
    assert all(r.returncode == 0 for r in results)
    path = tmp_path / "stats/events.jsonl"
    assert len(path.read_text().splitlines()) == 1
    record = json.loads(path.read_text())
    for key, value in [
        ("rating", 5),
        ("role", "wizard"),
        ("schema_version", True),
        ("ts", "yesterday"),
        ("sources", []),
        ("data", {"message_id": -1}),
        ("agent", {"name": "x"}),
    ]:
        bad = {**record, key: value}
        payload = {k: v for k, v in bad.items() if k != "id"}
        bad["id"] = hashlib.sha256(
            json.dumps(
                payload, ensure_ascii=False, sort_keys=True, separators=(",", ":")
            ).encode()
        ).hexdigest()
        path.write_text(json.dumps(bad) + "\n")
        assert call("validate", path).returncode == 1, (key, value)
    before = path.read_bytes()
    assert call(*args).returncode == 1
    assert path.read_bytes() == before


def test_handover_uses_existing_assignment_for_deleted_old_agent(tmp_path):
    state = tmp_path / "state.json"
    base = {
        "ok": True,
        "result": {
            "rooms": [{"id": 8, "orchestrator": 3}],
            "agents": [
                {"id": 2, "name": "old", "provider": "codex", "room_id": 8},
                {"id": 3, "name": "orch", "provider": "claude_code", "orchestrates": 8},
            ],
        },
    }
    state.write_text(json.dumps(base))
    common = (
        "--store",
        tmp_path / "stats",
        "--task",
        "pr-12",
        "--role",
        "author",
        "--orchestrator",
        "3",
        "--state",
        state,
    )
    assert call("assign", *common, "--agent", "2").returncode == 0
    base["result"]["agents"][0] = {
        "id": 4,
        "name": "new",
        "provider": "codex",
        "room_id": 8,
    }
    state.write_text(json.dumps(base))
    result = call(
        "handover",
        *common,
        "--agent",
        "4",
        "--old-agent",
        "2",
        "--reason",
        "compaction",
    )
    assert result.returncode == 0, result.stderr
    rows = [
        json.loads(x)
        for x in (tmp_path / "stats/events.jsonl").read_text().splitlines()
    ]
    assert rows[-1]["data"]["old_agent"]["name"] == "old"
    assert rows[-1]["agent"]["name"] == "new"


def test_assignment_preserves_snapshot_bytes_when_external_input_changes(tmp_path):
    import hashlib

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
    result = call(
        "assign",
        "--store",
        tmp_path / "stats",
        "--task",
        "x",
        "--role",
        "author",
        "--agent",
        "2",
        "--orchestrator",
        "3",
        "--state",
        state,
    )
    assert result.returncode == 0, result.stderr
    state.write_text("{}")
    row = json.loads((tmp_path / "stats/events.jsonl").read_text())
    for src in row["sources"]:
        assert (
            hashlib.sha256(Path(src["path"]).read_bytes()).hexdigest() == src["sha256"]
        )
