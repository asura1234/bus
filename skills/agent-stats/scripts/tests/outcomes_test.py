"""Stopping early needs idle, retry, no result and unchanged delivery artifacts."""

import json
import subprocess
import sys
from pathlib import Path

CLI = Path(__file__).resolve().parents[1] / "agent_stats.py"


def call(*args):
    return subprocess.run(
        [sys.executable, str(CLI), *map(str, args)], text=True, capture_output=True
    )


def test_stopped_early_requires_all_machine_evidence_and_retains_message_ids(tmp_path):
    for mutation in [
        "none",
        "reply",
        "sent",
        "working",
        "commit",
        "incomplete",
        "no-retry",
    ]:
        root = tmp_path / mutation
        root.mkdir()
        store = root / "stats"
        workspace = {
            "repo": str(root),
            "head": "a" * 40,
            "branch": "feature",
            "prs": [],
        }
        wp = root / "workspace.json"
        wp.write_text(json.dumps(workspace))
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
                        "status": "idle",
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
        sp = root / "state.json"
        sp.write_text(json.dumps(state))
        args = (
            "assign",
            "--store",
            store,
            "--task",
            "task-x",
            "--role",
            "author",
            "--agent",
            "2",
            "--orchestrator",
            "3",
            "--state",
            sp,
            "--workspace-snapshot",
            wp,
        )
        initial = call(*args, "--message", "100")
        assert initial.returncode == 0, initial.stderr
        first = json.loads(initial.stdout)["id"]
        if mutation != "no-retry":
            assert call(*args, "--message", "101").returncode == 0
        bundle = {
            "state": state,
            "status": {
                "ok": True,
                "result": {
                    "message_id": 100,
                    "requests": [
                        {
                            "agent_id": 2,
                            "turn_ended": True,
                            "reply": None,
                            "stage": "abandoned",
                        }
                    ],
                },
            },
            "history_pages": [
                {
                    "ok": True,
                    "result": {
                        "room_id": 8,
                        "messages": [{"prompt": {"id": 99, "author": {"agent": 3}}}],
                        "has_more": False,
                    },
                }
            ],
            "workspace": workspace,
        }
        if mutation == "reply":
            bundle["status"]["result"]["requests"][0]["reply"] = {"text": "Result"}
        if mutation == "sent":
            bundle["history_pages"][0]["result"]["messages"].append(
                {"prompt": {"id": 102, "author": {"agent": 2}}}
            )
        if mutation == "working":
            bundle["state"]["result"]["agents"][0]["status"] = "working"
        if mutation == "commit":
            bundle["workspace"] = {**workspace, "head": "b" * 40}
        if mutation == "incomplete":
            bundle["history_pages"][0]["result"].update(messages=[], has_more=True)
        proof = root / "proof.json"
        proof.write_text(json.dumps(bundle))
        end = call(
            "assignment-end",
            "--store",
            store,
            "--assignment",
            first,
            "--outcome",
            "stopped_early",
            "--evidence",
            proof,
        )
        assert end.returncode == 0, end.stderr
        report = call("report", "--store", store, "--json")
        assert report.returncode == 0, report.stderr
        worker = [
            r
            for r in json.loads(report.stdout)["leaderboard"]
            if r["agent"] == "worker"
        ][0]
        assert worker["stopped_early"] == (1 if mutation == "none" else 0)
        rows = [
            json.loads(x) for x in (store / "events.jsonl").read_text().splitlines()
        ]
        ends = [x for x in rows if x["kind"] == "assignment_end"]
        assert ends[0]["data"]["message_ids"] == (
            [100] if mutation == "no-retry" else [100, 101]
        )
        assert ends[0]["data"]["outcome"] == "stopped_early"
        assert ends[0]["data"]["proven"] == (mutation == "none")
