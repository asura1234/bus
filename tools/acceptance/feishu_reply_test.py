#!/usr/bin/env python3
"""Opt-in private fake-bridge acceptance; importing starts no processes or sessions."""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import traceback
import uuid

ROOT = Path(__file__).resolve().parents[2]
APP = "cli_test"
RECIPIENT = "ou_fixture_developer"
FAKE_BRIDGE = r'''#!/usr/bin/python3
import fcntl, json, os, pathlib, sys, time
root = pathlib.Path(__file__).resolve().parent
args = sys.argv[1:]
assert len(args) == 7 and args[0] == "send", "v1 send argv required"
assert args[1] == "--open-id" and args[3] == "--text" and args[5] == "--idempotency-key"
state_path = pathlib.Path(os.environ["BUS_DATA_DIR"]) / "state.json"
state = json.loads(state_path.read_text())["state"]
records = [record for record in state.get("feishu", {}).get("outbox", {}).values() if record["key"] == args[6]]
assert len(records) == 1, "outbox must be persisted before bridge launch"
record = records[0]
assert record["attempted"] and record["message_id"] is None
assert record["recipient"] == args[2] and record["text"] == args[4]
assert record["session"] == str(state_path.parent)
assert record["raising_agent"] is not None and record["orchestrator"] is not None
assert 1 <= len(record["key"]) <= 50 and record["key"].isascii()
assert "bus:" + record["code"] in args[4]
with (root / "calls.jsonl").open("a+") as ledger:
    fcntl.flock(ledger, fcntl.LOCK_EX)
    ledger.seek(0)
    message_id = "om_fixture_" + str(len(ledger.readlines()) + 1)
    entry = {"argv": args, "message_id": message_id, "chat_id": "oc_dm", "outbox_verified": True,
             "outbox": record, "state_path": str(state_path), "at_ms": int(time.time() * 1000)}
    ledger.write(json.dumps(entry, ensure_ascii=False) + "\n")
    ledger.flush()
    os.fsync(ledger.fileno())
mode = root / "bridge-mode"
if mode.exists() and mode.read_text().strip() == "fail":
    print("synthetic bridge failure", file=sys.stderr)
    sys.exit(7)
print(json.dumps({"message_id": message_id, "chat_id": "oc_dm"}))
'''


def until(probe, description, timeout=35):
    deadline = time.monotonic() + timeout
    while True:
        value = probe()
        if value:
            return value
        if time.monotonic() >= deadline:
            raise AssertionError(description)
        time.sleep(0.1)


def json_file(path):
    return json.loads(Path(path).read_text())


class Run:
    def __init__(self, binary, name, bridge_binary=None):
        self.binary = Path(binary).resolve(strict=True)
        self.bridge_binary = Path(bridge_binary).resolve(strict=True) if bridge_binary else None
        slug = "".join(c if c.isalnum() or c in "-_" else "-" for c in name)[:32] or "run"
        self.artifacts = ROOT / "temp/feishu-integration/acceptance" / (slug + "-" + uuid.uuid4().hex[:8])
        self.artifacts.mkdir(parents=True, mode=0o700)
        # Darwin's native socket paths cannot fit beneath the worktree artifact path.
        # This owned short root is retained, with its location in both reports.
        self.runtime = Path(tempfile.mkdtemp(prefix="bfi-", dir="/private/tmp"))
        self.runtime.chmod(0o700)
        self.data, self.bin, self.work = (self.runtime / child for child in ("data", "bin", "work"))
        for directory in (self.data, self.bin, self.work, self.runtime / "home", self.runtime / "tmp"):
            directory.mkdir(mode=0o700)
        self.bridge = self.bin / "feishu-bridge"
        self.bridge.write_text(FAKE_BRIDGE)
        self.bridge.chmod(0o700)
        for executable in ("claude", "codex", "open", "osascript", "terminal-notifier", "afplay"):
            (self.bin / executable).symlink_to(self.binary)
        sentinel = self.bin / "lark-cli"
        sentinel.write_text('#!/bin/sh\nprintf forbidden > "${0%/*}/lark-was-called"\nexit 97\n')
        sentinel.chmod(0o700)
        self.env = {
            "PATH": f"{self.bin}:/usr/bin:/bin:/usr/sbin:/sbin", "HOME": str(self.runtime / "home"),
            "USERPROFILE": str(self.runtime / "home"), "SHELL": "/bin/bash", "TERM": "xterm-256color",
            "TMPDIR": str(self.runtime / "tmp"), "XDG_CONFIG_HOME": str(self.runtime / "xdg"),
            "BUS_DATA_DIR": str(self.data), "BUS_TUI_FAKE_AGENT": "1", "BUS_TUI_OWNER": uuid.uuid4().hex,
            "BUS_TUI_EFFECTS": str(self.runtime / "effects.jsonl"), "BUS_DISABLE_SOUND": "1",
            "TEST_BUS_PROTECTED_HOME": str(Path.home()),
        }
        self.serial, self.bus = 0, None
        self.report = {"result": "FAIL", "binary": str(self.binary), "runtime_root": str(self.runtime),
                       "artifacts": str(self.artifacts), "cases": [], "cleanup": [],
                       "limits": ["Fake providers and bridge; no real Feishu send or developer reply."]}
        if not bridge_binary:
            self.report["limits"].append("Real feishu-bridge synthetic import was not requested.")
        self.settings(feishu_recipient=RECIPIENT, feishu_bridge_path=str(self.bridge))

    def record(self, label, value):
        self.serial += 1
        path = self.artifacts / f"{self.serial:03d}-{label}.json"
        path.write_text(json.dumps(value, indent=2, ensure_ascii=False))
        return str(path)

    def settings(self, **changes):
        path = self.data / "settings.json"
        value = json_file(path) if path.exists() else {}
        value.update(changes)
        temporary = path.with_suffix(".acceptance-tmp")
        temporary.write_text(json.dumps(value))
        temporary.replace(path)

    def state(self):
        return json_file(self.data / "state.json")["state"]

    def cli(self, *args):
        result = self.bus.cli(*map(str, args))
        self.record("control", {"argv": list(map(str, args)), "result": result})
        return result

    def case(self, name, action):
        row = {"name": name, "result": "FAIL"}
        self.report["cases"].append(row)
        action()
        row["result"] = "PASS"

    def calls(self):
        path = self.bin / "calls.jsonl"
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def quiet(self):
        time.sleep(2.2)
        self.cli("state")

    def verify_identity(self):
        head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
        identity = subprocess.run([str(self.binary), "--identity"], env=self.env, capture_output=True, text=True, timeout=10, check=True)
        compiled = json.loads(identity.stdout)
        assert compiled["sha"] == head, (head, compiled)
        assert os.access(self.binary, os.X_OK)
        self.report.update(head=head, identity=compiled, binary_sha256=hashlib.sha256(self.binary.read_bytes()).hexdigest())
        for name in ("claude", "codex"):
            assert shutil.which(name, path=self.env["PATH"]) == str(self.bin / name)
            assert (self.bin / name).resolve() == self.binary
        assert self.env["HOME"] != str(Path.home())
        assert not self.data.is_relative_to(Path.home() / ".local/share/bus")
        sys.path.insert(0, str(ROOT))
        from tools.acceptance.harness_test import Bus
        self.bus = Bus(self.binary, self.runtime, self.env, workspace=self.work)
        state = self.bus.start()
        self.check_build(state)
        shown = self.cli("settings")["settings"]
        assert shown["feishu_bridge_path"] == str(self.bridge) and "feishu_cli_path" not in shown
        until(lambda: self.cli("settings")["settings"]["feishu"]["enabled"], "optional fake bridge did not enable")
        self.orch_a = self.add_orchestrator("orch-a", "work-a")
        self.orch_b = self.add_orchestrator("orch-b", "work-b")
        self.worker = self.cli("agent", "add", "--room", "work-a", "--name", "worker-a", "--provider", "claude",
                               "--pwd", self.work, "--consent-hooks")["agent_id"]
        until(lambda: all(agent["status"] == "idle" for agent in self.cli("state")["agents"]), "fake agents did not become idle")

    def check_build(self, state):
        build = state["build"]
        for role in ("client", "server"):
            assert build[role]["sha"] == self.report["head"], build
            assert Path(build[role]["binary"]).resolve() == self.binary, build
        self.record("build", build)

    def add_orchestrator(self, name, room):
        if not any(row["name"] == room for row in self.cli("state")["rooms"]):
            self.cli("room", "create", room)
        return self.cli("agent", "add", "--room", "master", "--name", name, "--provider", "claude",
                        "--pwd", self.work, "--orchestrates", room, "--consent-hooks")["agent_id"]

    def post(self, room, author, text):
        return self.cli("send", "--room", room, "--as", author, "--to", "you", "--text", text)["message_id"]

    def outbound(self):
        self.post("master", self.orch_a, "Routine progress")
        self.post("master", self.orch_a, "[needs_developer_attention] Internal only")
        self.quiet()
        assert not self.calls(), self.calls()
        sources = [
            self.post("master", self.orch_a, "[waiting-on-you] First decision 界"),
            self.post("master", self.orch_b, "[waiting_on_you] Second decision"),
            self.post("work-a", self.worker, "[waiting-on-you] Same decision"),
        ]
        until(lambda: sum(record["message_id"] is not None for record in self.state()["feishu"]["outbox"].values()) == 3,
              "initial DMs must be confirmed before suppressing the repeat")
        sources.append(self.post("work-a", self.worker, "[waiting-on-you] Same decision"))
        until(lambda: len(self.calls()) == 3, "four requests did not share three DMs")
        until(lambda: sum(record["message_id"] is not None for record in self.state()["feishu"]["outbox"].values()) == 3,
              "returned message IDs were not persisted")
        self.quiet()
        calls = self.calls()
        assert len(calls) == 3 and all(call["outbox_verified"] for call in calls), calls
        saved = self.state()["feishu"]["outbox"]
        self.routes = {route["source"]["prompt_id"]: dict(call, outbox=dict(record, **route))
                       for call in calls for record in [saved[call["outbox"]["code"]]]
                       for route in [record, *record.get("aliases", [])]}
        assert set(self.routes) == set(sources), (self.routes, sources)
        assert len({call["outbox"]["code"] for call in calls}) == 3
        assert len({call["outbox"]["key"] for call in calls}) == 3
        assert all(call["argv"][2] == RECIPIENT for call in calls)
        self.first, self.second, self.same_first, self.same_second = [self.routes[source] for source in sources]
        assert self.same_first["outbox"]["raising_agent"] == self.worker
        assert self.same_second["outbox"]["orchestrator"] == self.orch_a
        assert self.same_first["message_id"] == self.same_second["message_id"]
        assert "Second decision" not in self.first["argv"][4]
        self.record("outbound-ledger", calls)

    def event(self, message_id, parent=None, root=None, text="Proceed", **changes):
        value = {"app_id": APP, "message_id": message_id, "chat_id": "oc_dm", "chat_type": "p2p",
                 "sender": {"open_id": RECIPIENT, "sender_type": "user"}, "text": text,
                 "reply_to": parent, "root_id": root, "thread_id": None, "create_time": str(int(time.time() * 1000))}
        value.update(changes)
        return value

    def ingest(self, event, accepted=True):
        # Deliberately wrong, private inherited identity/root: --instance must own routing.
        env = dict(self.env, BUS_DATA_DIR=str(self.runtime / "wrong-instance"), BUS_AGENT_ID=str(self.worker))
        result = subprocess.run([str(self.binary), "feishu", "ingest", "--instance", str(self.data), "--app-id", APP],
                                input=json.dumps(event) + "\n", env=env, capture_output=True, text=True, timeout=45)
        self.record("ingest", {"event": event, "exit": result.returncode, "stdout": result.stdout, "stderr": result.stderr})
        if not accepted:
            assert result.returncode != 0, result.stdout
            return None
        assert result.returncode == 0, (result.stdout, result.stderr)
        receipt = json.loads(result.stdout)
        assert receipt["ok"], receipt
        return receipt["result"]

    def requests_for(self, external_id):
        return [request for request in self.state()["requests"].values()
                if request["prompt"].get("in_reply_to", {}).get("feishu_message_id") == external_id]

    def assert_delivery(self, event, receipt, route, target):
        assert receipt["stage"] == "delivered", receipt
        saved = route["outbox"]
        matches = [request for request in self.requests_for(event["message_id"])
                   if request["prompt"]["in_reply_to"]["source"] == saved["source"]]
        assert len(matches) == 1, matches
        message = matches[0]["prompt"]["id"]
        status = until(lambda: self.complete_status(message), f"fake provider did not complete Feishu message {message}")
        assert len(status["requests"]) == 1, status
        request = status["requests"][0]
        assert request["agent_id"] == target and request["queue"] is True, request
        assert request["start_bound"] and request["latency"]["provider_started_at_ms"] is not None, request
        assert request["stage"] == "ended" and request["turn_id"], request
        prompt = status["prompt"]
        assert prompt["author"] == "human" and prompt["text"] == event["text"], prompt
        provenance = prompt["in_reply_to"]
        assert provenance["source"] == saved["source"] and provenance["raising_agent"] == saved["raising_agent"]
        assert provenance["feishu_message_id"] == event["message_id"] and provenance["orchestrator"] == target
        self.record("delivered-provider-turn", status)
        return request["turn_id"]

    def complete_status(self, message):
        status = self.cli("message", "status", message)
        return status if status["complete"] else None

    def inbound(self):
        self.replay_event = self.event("om_in_first", parent=self.first["message_id"], text="fake:work 3 FEISHU_FIRST")
        self.replay_receipt = self.ingest(self.replay_event)
        until(lambda: self.cli("message", "status", self.replay_receipt["message_id"])["requests"][0]["start_bound"],
              "first authenticated reply never started")
        worker_event = self.event("om_in_worker", parent=self.same_first["message_id"], text="FEISHU_WORKER_REPLY")
        worker_receipt = self.ingest(worker_event)
        first_turn = self.assert_delivery(self.replay_event, self.replay_receipt, self.first, self.orch_a)
        second_turn = self.assert_delivery(worker_event, worker_receipt, self.same_first, self.orch_a)
        alias_turn = self.assert_delivery(worker_event, worker_receipt, self.same_second, self.orch_a)
        assert len({first_turn, second_turn, alias_turn}) == 3, "every logical source gets its own provider turn"
        assert len(self.requests_for(worker_event["message_id"])) == 2
        call_count = len(self.calls())
        late_source = self.post("work-a", self.worker, "[waiting-on-you] Same decision")
        until(lambda: len(self.requests_for(worker_event["message_id"])) == 3,
              "late alias must receive the cached reply before any Feishu replay")
        mapping = self.state()["feishu"]["outbox"][self.same_first["outbox"]["code"]]
        late = next(route for route in mapping["aliases"] if route["source"]["prompt_id"] == late_source)
        late_route = dict(self.same_first, outbox=dict(mapping, **late))
        late_turn = self.assert_delivery(worker_event, worker_receipt, late_route, self.orch_a)
        assert len({first_turn, second_turn, alias_turn, late_turn}) == 4
        assert len(self.calls()) == call_count, "a late alias cannot send another DM"
        self.worker_routes = [self.same_first, self.same_second, late_route]
        assert self.ingest(worker_event) == worker_receipt
        assert len(self.requests_for(worker_event["message_id"])) == 3
        assert self.ingest(self.replay_event) == self.replay_receipt
        changed = dict(self.replay_event, text="CHANGED_REPLAY_MUST_NOT_RUN")
        assert self.ingest(changed) == self.replay_receipt
        for event, route, target in [
            (self.event("om_in_root", parent="om_unknown_parent", root=self.second["message_id"], text="FEISHU_ROOT_REPLY"), self.second, self.orch_b),
            (self.event("om_in_code", text="bus:" + self.same_second["outbox"]["code"] + " FEISHU_CODE_REPLY"), self.same_second, self.orch_a),
            (self.event("om_in_nested", parent="om_in_worker", text="FEISHU_NESTED_REPLY"), self.same_first, self.orch_a),
        ]:
            receipt = self.ingest(event)
            logical_routes = self.worker_routes if route["outbox"]["code"] == self.same_first["outbox"]["code"] else [route]
            turns = {self.assert_delivery(event, receipt, logical, target) for logical in logical_routes}
            assert len(turns) == len(logical_routes)
        assert len(self.requests_for("om_in_first")) == 1

    def refusal(self):
        before = len(self.state()["requests"])
        for event in [self.event("om_unknown", text="Unmapped"),
                      self.event("om_wrong_chat", parent=self.first["message_id"], chat_id="oc_other"),
                      self.event("om_ambiguous", parent=self.first["message_id"], root=self.second["message_id"]),
                      self.event("om_ambiguous_codes", text="bus:" + self.first["outbox"]["code"] + " bus:" + self.second["outbox"]["code"])]:
            assert self.ingest(event)["stage"] == "parked"
            assert not self.requests_for(event["message_id"])
            assert APP + ":" + event["message_id"] in self.state()["feishu"]["inbox"]
        for event in [self.event("om_wrong_sender", parent=self.first["message_id"], sender={"open_id":"ou_impostor","sender_type":"user"}),
                      self.event("om_bot", parent=self.first["message_id"], sender={"open_id":RECIPIENT,"sender_type":"app"}),
                      self.event("om_group", parent=self.first["message_id"], chat_type="group"),
                      self.event("om_wrong_app", parent=self.first["message_id"], app_id="cli_other")]:
            self.ingest(event, accepted=False)
            assert not self.requests_for(event["message_id"])
            assert APP + ":" + event["message_id"] not in self.state()["feishu"]["inbox"]
        assert len(self.state()["requests"]) == before
        self.cli("agent", "delete", self.orch_b, "--confirm")
        replacement = self.add_orchestrator("orch-b", "work-b")
        assert replacement != self.orch_b
        parked = self.event("om_deleted_target", parent=self.second["message_id"])
        assert self.ingest(parked)["stage"] == "parked"
        assert not self.requests_for("om_deleted_target")
        self.no_popup()

    def no_popup(self):
        path = self.runtime / "effects.jsonl"
        effects = [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []
        assert not [row for row in effects if row.get("kind") in ("notification", "terminal_notification")], effects
        screen = (self.runtime / "tui.log").read_bytes()
        assert not any(marker in screen for marker in (b"\x1b]9;", b"\x1b]99;", b"\x1b]777;"))
        assert b"feishu-bridge failed" not in screen and b"Feishu sender does not match" not in screen

    def plugin_off_and_failure(self):
        count = len(self.calls())
        self.settings(feishu_bridge_path=str(self.bin / "not-installed"))
        until(lambda: not self.cli("settings")["settings"]["feishu"]["enabled"], "missing bridge did not disable Feishu")
        self.post("master", self.orch_a, "[waiting-on-you] Missing plugin")
        self.quiet()
        assert len(self.calls()) == count
        self.settings(feishu_bridge_path=str(self.bridge))
        until(lambda: self.cli("settings")["settings"]["feishu"]["enabled"], "restored fake plugin did not enable")
        self.quiet()
        assert len(self.calls()) == count, "restoring a plugin cannot replay off-period alerts"
        (self.bin / "bridge-mode").write_text("fail")
        self.post("master", self.orch_a, "[waiting-on-you] Synthetic send failure")
        until(lambda: len(self.calls()) == count + 1, "failed fake bridge was never invoked")
        until(lambda: self.cli("settings")["settings"]["feishu"]["error"], "synthetic send failure was not recorded")
        self.quiet()
        assert len(self.calls()) == count + 1, "uncertain failed send cannot auto-retry"
        logs = "\n".join(path.read_text(errors="replace") for path in self.data.rglob("*.log"))
        assert "bus.feishu.failed" in logs and "exit code Some(7)" in logs
        assert "bus.feishu.parked" in logs
        self.no_popup()
        (self.bin / "bridge-mode").unlink()

    def synthetic_bridge(self):
        if self.bridge_binary is None:
            return
        state_dir = self.runtime / "bridge-spool"
        config = self.runtime / "bridge-synthetic.toml"
        config.write_text("\n".join([
            'app_id = "cli_test"', "lark_cli = " + json.dumps(str(self.bin / "lark-cli")),
            "state_dir = " + json.dumps(str(state_dir)), "allowed_sender_open_ids = " + json.dumps([RECIPIENT]),
            'allowed_chat_ids = ["oc_dm"]', "sink_command = " + json.dumps([str(self.binary), "feishu", "ingest", "--instance", str(self.data), "--app-id", APP]),
        ]) + "\n")
        path = self.runtime / "synthetic.ndjson"
        event = {"type":"im.message.receive_v1", "event_id":"ev_synthetic", "message_id":"om_real_bridge_synthetic", "chat_id":"oc_dm", "chat_type":"p2p",
                 "sender_id":RECIPIENT, "sender_type":"user", "message_type":"text", "content":"FEISHU_SYNTHETIC_BRIDGE_REPLY",
                 "reply_to":self.first["message_id"], "root_id":None, "thread_id":None, "create_time":str(int(time.time()*1000))}
        path.write_text(json.dumps(event) + "\n")
        for replay in (False, True):
            result = subprocess.run([str(self.bridge_binary), "--config", str(config), "listen", "--input", str(path)],
                                    env=self.env, capture_output=True, text=True, timeout=45)
            self.record("real-bridge-synthetic", {"replay":replay,"exit":result.returncode,"stdout":result.stdout,"stderr":result.stderr})
            assert result.returncode == 0, result.stderr
        assert not (self.bin / "lark-was-called").exists(), "synthetic import must never call lark-cli"
        input_record = self.state()["feishu"]["inbox"][APP + ":om_real_bridge_synthetic"]
        normalized = self.event("om_real_bridge_synthetic", parent=self.first["message_id"], text=event["content"])
        self.assert_delivery(normalized, input_record["outcome"], self.first, self.orch_a)

    def restart_dedupe(self):
        count = len(self.calls())
        before = copy.deepcopy(self.requests_for("om_in_first"))
        old_client_pid = self.bus.child.pid
        old_server_pid = self.cli("state")["build"]["server"]["pid"]
        cleanup = self.bus.stop()
        self.report["cleanup"].append(cleanup)
        assert not cleanup["leftover"], cleanup
        assert self.bus.child.poll() is not None, "scratch coordinator did not stop"
        self.bus.pump_thread.join(timeout=5)
        assert not self.bus.pump_thread.is_alive(), "old scratch PTY reader did not stop"
        state = self.bus.start()
        self.check_build(state)
        assert self.bus.child.pid != old_client_pid
        assert state["build"]["server"]["pid"] != old_server_pid, "scratch native server did not restart"
        assert self.ingest(self.replay_event) == self.replay_receipt
        self.quiet()
        assert self.requests_for("om_in_first") == before
        assert len(self.calls()) == count, "restart cannot resend attempted outbox entries"
        self.no_popup()

    def finish(self):
        if self.bus is not None:
            try:
                cleanup = self.bus.stop()
                self.report["cleanup"].append(cleanup)
                if cleanup["leftover"]:
                    self.report["result"] = "FAIL"
            except Exception as error:
                self.report["cleanup_error"] = str(error)
                self.report["result"] = "FAIL"
        self.report["bridge_calls"] = self.calls()
        self.report["finished_at"] = time.strftime("%Y-%m-%dT%H:%M:%S%z")
        (self.artifacts / "report.json").write_text(json.dumps(self.report, indent=2, ensure_ascii=False))
        rows = [f"# Feishu bridge acceptance: {self.report['result']}", "", f"Head: `{self.report.get('head', 'unknown')}`",
                f"Runtime retained: `{self.runtime}`", f"Binary: `{self.binary}`", ""]
        rows += [f"- {case['result']}: {case['name']}" for case in self.report["cases"]]
        rows += ["", *[f"- Limit: {limit}" for limit in self.report["limits"]]]
        if "error" in self.report:
            rows += ["", "Failure: " + self.report["error"]]
        (self.artifacts / "report.md").write_text("\n".join(rows) + "\n")
        print(f"{self.report['result']}: {self.artifacts / 'report.md'}", flush=True)
        return 0 if self.report["result"] == "PASS" else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path, help="Absolute current-head dev-tools Bus binary")
    parser.add_argument("--name", default="feishu-bridge", help="Artifact run-name prefix")
    parser.add_argument("--bridge-binary", type=Path, help="Optional real bridge; only listen --input with a private generated config is used")
    args = parser.parse_args()
    if not args.binary.is_absolute() or args.bridge_binary is not None and not args.bridge_binary.is_absolute():
        parser.error("executable paths must be absolute")
    for executable in (args.binary, args.bridge_binary):
        if executable is not None and (not executable.is_file() or not os.access(executable, os.X_OK)):
            parser.error(f"executable not found: {executable}")
    os.umask(0o077)
    run = Run(args.binary, args.name, args.bridge_binary)
    try:
        run.case("isolation and exact build identity", run.verify_identity)
        run.case("durable one-DM-per-request outbox", run.outbound)
        run.case("correlated developer messages reach independent provider turns", run.inbound)
        if run.bridge_binary is not None:
            run.case("real bridge synthetic import and replay without transport", run.synthetic_bridge)
        run.case("unknown, ambiguous, unauthorized and deleted-target replies", run.refusal)
        run.case("optional plugin off and log-only send failure", run.plugin_off_and_failure)
        run.case("restart preserves inbound and outbound deduplication", run.restart_dedupe)
        run.report["result"] = "PASS"
    except Exception as error:
        run.report["error"] = str(error)
        (run.artifacts / "failure.txt").write_text(traceback.format_exc())
    return run.finish()


if __name__ == "__main__":
    raise SystemExit(main())
