"""Receipt attribution checks against the recovery exporter's CLI."""
import json
from pathlib import Path
import subprocess
import sys


EXPORTER = Path(__file__).resolve().parents[1] / "deleted_messages.py"


def run_exporter(tmp_path, command, responses):
    evidence = tmp_path / "copied-evidence"
    evidence.mkdir()
    output = tmp_path / "recovered"
    events = [
        {
            "type": "response_item",
            "payload": {
                "type": "function_call",
                "name": "exec_command",
                "call_id": "shell-1",
                "arguments": json.dumps({"cmd": command}),
            },
        },
        {
            "type": "response_item",
            "payload": {
                "type": "function_call_output",
                "call_id": "shell-1",
                "output": json.dumps(
                    {"output": "\n".join(json.dumps(reply) for reply in responses)}
                ),
            },
        },
    ]
    (evidence / "transcript.jsonl").write_text(
        "\n".join(json.dumps(event) for event in events) + "\n"
    )
    completed = subprocess.run(
        [sys.executable, str(EXPORTER), "--evidence", str(evidence), "--output", str(output)],
        check=True,
        capture_output=True,
        text=True,
    )
    summary = json.loads(completed.stdout)
    return [json.loads((output / name).read_text()) for name in summary["files"]]


def test_send_receipt_links_authored_text(tmp_path):
    exports = run_exporter(
        tmp_path,
        "bus send --room 877 --as 21829 --to 23485 --text 'verified send body'",
        [{"ok": True, "result": {"message_id": 22193, "request_ids": [22194], "stage": "queued"}}],
    )
    accepted = [entry for entry in exports if entry["kind"] == "receipt_linked_send"]
    assert len(accepted) == 1
    assert accepted[0]["receipt"]["message_id"] == 22193
    assert accepted[0]["record"]["text"] == "verified send body"


def test_status_message_id_never_becomes_a_send_receipt(tmp_path):
    exports = run_exporter(
        tmp_path,
        "bus send --room 877 --as 21829 --to 23485 --text 'verified send body'\n"
        "bus message status 22223",
        [
            {"ok": True, "result": {"message_id": 22193, "request_ids": [22194], "stage": "queued"}},
            {
                "ok": True,
                "result": {
                    "message_id": 22223,
                    "complete": True,
                    "waiting_on_dialog": [],
                    "requests": [],
                },
            },
        ],
    )
    accepted = [entry for entry in exports if entry["kind"] == "receipt_linked_send"]
    assert {entry["receipt"]["message_id"] for entry in accepted} == {22193}, (
        "a status response must not attach this send's text to another message ID"
    )


def test_posted_send_with_no_requests_still_confirms_authored_text(tmp_path):
    exports = run_exporter(
        tmp_path,
        "bus send --room 877 --as 21829 --to you --text 'developer report'",
        [{"ok": True, "result": {"message_id": 22193, "request_ids": [], "stage": "posted"}}],
    )
    accepted = [entry for entry in exports if entry["kind"] == "receipt_linked_send"]
    assert len(accepted) == 1
    assert accepted[0]["receipt"]["message_id"] == 22193
    assert accepted[0]["record"]["text"] == "developer report"


def test_failed_send_followed_by_status_retains_only_unconfirmed_intent(tmp_path):
    exports = run_exporter(
        tmp_path,
        "bus send --room 877 --as 21829 --to 23485 --text 'unaccepted body'\n"
        "bus message status 22223",
        [
            {"ok": False, "error": {"message": "recipient unavailable"}},
            {"ok": True, "result": {"message_id": 22223, "requests": [], "complete": True}},
        ],
    )
    assert not any(entry["kind"] == "receipt_linked_send" for entry in exports)
    unconfirmed = [entry for entry in exports if entry["kind"] == "unconfirmed_send_command"]
    assert len(unconfirmed) == 1
    assert unconfirmed[0]["record"]["text"] == "unaccepted body"
