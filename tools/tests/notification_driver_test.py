"""Shared notification acceptance tooling stays independent of the bridge scenario."""
import importlib
import json
import os
from pathlib import Path
import subprocess
from types import SimpleNamespace


def test_notification_driver_preserves_child_report_docs_and_driver_command_forwarding(tmp_path, monkeypatch):
    helper = importlib.import_module("tools.acceptance.notification_driver")
    sound = importlib.import_module("tools.acceptance.developer_sound_test")
    notify = importlib.import_module("tools.acceptance.notify_you_test")
    binary = tmp_path / "bus"
    binary.write_bytes(b"fake binary fingerprint")
    monkeypatch.setattr(helper, "ROOT", tmp_path)
    reports = []

    def output(argv, **_):
        if argv[:3] == ["git", "rev-parse", "HEAD"]:
            return "1" * 40 + "\n"
        if argv[:2] == ["git", "diff"]:
            return b""
        if argv[:3] == ["git", "ls-files", "--others"]:
            return b""
        assert "init" in argv and "--doc" in argv, argv
        doc = argv[argv.index("--doc") + 1]
        reports.append(doc)
        report = tmp_path / str(len(reports)) / "report.md"
        report.parent.mkdir()
        report.write_text("report fixture")
        return str(report) + "\n"

    monkeypatch.setattr(helper.subprocess, "check_output", output)
    sound_run = sound.Run(binary, "sound")
    notify_run = notify.Run(binary, "notify")
    assert reports == ["tests/e2e-tui/developer-sound.md", "tests/e2e-tui/notify-you.md"]
    assert sound_run.doc == reports[0] and notify_run.doc == reports[1]
    assert isinstance(sound_run.report, Path) and isinstance(notify_run.report, Path)
    seen = []

    def execute(argv, **kwargs):
        seen.append((argv, kwargs))
        if "--queue" in argv:
            return SimpleNamespace(returncode=1, stdout='{"ok":false,"error":{"code":"command_failed"}}', stderr="")
        return SimpleNamespace(returncode=0, stdout='{"ok":true,"result":{"marker":"forwarded"}}', stderr="")

    monkeypatch.setattr(helper.subprocess, "run", execute)
    assert notify_run.bus("send", "--text", "literal $HOME") == {"ok": True, "result": {"marker": "forwarded"}}
    assert seen[0][0] == [str(binary.resolve()), "--dev", "tui", "bus", "--name", notify_run.name, "send", "--text", "literal $HOME"]
    assert seen[0][1]["env"]["PATH"].startswith(str(notify_run.bin) + ":")
    assert json.loads((notify_run.root / "001.json").read_text())["status"] == 0
    assert (sound_run.root / "candidate.txt").is_file()
    refused = notify_run.post_room("work", "worker", "you", "request", "--queue", ok=False)
    assert refused == {"ok": False, "error": {"code": "command_failed"}}
    assert json.loads((notify_run.root / "002.json").read_text())["status"] == 1


def test_notification_driver_fake_bridge_checks_the_actual_bus_data_dir_before_accepting(tmp_path):
    helper = importlib.import_module("tools.acceptance.notification_driver")
    script = tmp_path / "feishu-bridge"
    script.write_text(helper.FAKE_BRIDGE)
    script.chmod(0o700)
    data = tmp_path / "actual-scratch-data"
    data.mkdir()
    state = data / "state.json"
    state.write_text('{"state":{"feishu":{"outbox":{}}}}')
    argv = [str(script), "send", "--open-id", "ou_fixture", "--text", "Literal bus:abc", "--idempotency-key", "bus-stable"]
    env = dict(os.environ, BUS_DATA_DIR=str(data))
    missing = subprocess.run(argv, env=env, capture_output=True, text=True)
    assert missing.returncode != 0
    assert not (tmp_path / "calls.jsonl").exists()
    record = {"code":"abc","key":"bus-stable","attempted":True,"message_id":None,"recipient":"ou_fixture",
              "text":"Literal bus:abc","session":str(data),"source":{"prompt_id":7,"request_id":None},
              "raising_agent":4,"orchestrator":None}
    state.write_text(json.dumps({"state":{"feishu":{"outbox":{"abc":record}}}}))
    accepted = subprocess.run(argv, env=env, capture_output=True, text=True)
    assert accepted.returncode == 0, accepted.stderr
    assert json.loads(accepted.stdout) == {"message_id":"om_fixture_1","chat_id":"oc_dm"}
    call = json.loads((tmp_path / "calls.jsonl").read_text())
    assert call["state_path"] == str(state)
    assert call["outbox"]["source"]["prompt_id"] == 7
