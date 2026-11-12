"""Shared scratch-TUI notification driver; importing starts no processes."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
REPORT_TOOL = ROOT / "skills/e2e-test/scripts/e2e_report.py"
FAKE_BRIDGE = r'''#!/usr/bin/python3
import fcntl, json, os, pathlib, sys
root = pathlib.Path(__file__).resolve().parent
args = sys.argv[1:]
assert len(args) == 7 and args[0:2] == ["send", "--open-id"]
assert args[3] == "--text" and args[5] == "--idempotency-key"
assert 1 <= len(args[6]) <= 50 and args[6].isascii()
state_path = pathlib.Path(os.environ["BUS_DATA_DIR"]) / "state.json"
state = json.loads(state_path.read_text())["state"]
records = [r for r in state.get("feishu", {}).get("outbox", {}).values() if r["key"] == args[6]]
assert len(records) == 1, "outbox/key must be durable before bridge launch"
record = records[0]
assert record["attempted"] and record["message_id"] is None
assert record["recipient"] == args[2] and record["text"] == args[4]
assert record["session"] == str(state_path.parent)
assert "bus:" + record["code"] in args[4]
with (root / "calls.jsonl").open("a+") as ledger:
    fcntl.flock(ledger, fcntl.LOCK_EX)
    ledger.seek(0)
    message = "om_fixture_" + str(len(ledger.readlines()) + 1)
    ledger.write(json.dumps({"argv":args,"message_id":message,"chat_id":"oc_dm",
        "state_path":str(state_path),"outbox_verified":True,"outbox":record}) + "\n")
    ledger.flush()
    os.fsync(ledger.fileno())
print(json.dumps({"message_id":message,"chat_id":"oc_dm"}))
'''


class Run:
    doc = None

    def __init__(self, binary, name, *, doc=None):
        self.doc = doc or self.doc
        if not self.doc:
            raise ValueError("a child acceptance doc is required")
        self.binary = Path(binary).resolve()
        self.name = "notify-" + hashlib.sha256(name.encode()).hexdigest()[:8]
        self.env = {key: value for key, value in os.environ.items()
                    if not key.startswith(("BUS_", "HERDR_", "CLAUDE", "XDG_"))
                    and key not in ("CODEX_HOME", "CLAUDE_CONFIG_DIR")}
        commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
        self.report = Path(subprocess.check_output([
            sys.executable, str(REPORT_TOOL), "init", "--doc", self.doc, "--name", name,
            "--runs-root", str(ROOT / "temp/notification-driver"), "--commit", commit,
            "--platform", "macOS scratch driver 160x55, graphics none",
        ], cwd=ROOT, text=True).strip())
        self.root = self.report.parent
        self.bin = self.root / "fake-bin"
        self.bin.mkdir()
        self.cli = self.bin / "feishu-bridge"
        self.cli.write_text(FAKE_BRIDGE)
        self.cli.chmod(0o700)
        self.env["PATH"] = str(self.bin) + ":/usr/bin:/bin:/usr/sbin:/sbin"
        assert shutil.which("feishu-bridge", path=self.env["PATH"]) == str(self.cli)
        delta = subprocess.check_output(["git", "diff", "HEAD"], cwd=ROOT)
        fingerprint = hashlib.sha256(delta)
        untracked = subprocess.check_output(["git", "ls-files", "--others", "--exclude-standard", "-z"], cwd=ROOT)
        for entry in sorted(untracked.split(b"\0")):
            if entry:
                fingerprint.update(entry + b"\0" + (ROOT / os.fsdecode(entry)).read_bytes())
        binary_hash = hashlib.sha256(self.binary.read_bytes()).hexdigest()
        (self.root / "candidate.txt").write_text(f"HEAD {commit}\ndelta-sha256 {fingerprint.hexdigest()}\nbinary {self.binary}\nbinary-sha256 {binary_hash}\n")
        self.serial, self.session = 0, None

    def command(self, *args, ok=True):
        self.serial += 1
        out = subprocess.run([str(self.binary), "--dev", "tui", args[0], "--name", self.name, *map(str, args[1:])],
                             cwd=ROOT, env=self.env, capture_output=True, text=True, timeout=45)
        (self.root / f"{self.serial:03d}.json").write_text(json.dumps({"argv": list(map(str, args)), "status": out.returncode, "stdout": out.stdout, "stderr": out.stderr}, indent=2))
        if ok:
            assert out.returncode == 0, (args, out.stdout, out.stderr)
        try:
            return json.loads(out.stdout)
        except json.JSONDecodeError:
            return {"text": out.stdout, "status": out.returncode}

    def bus(self, *args, ok=True):
        value = self.command("bus", *args, ok=ok)
        if ok:
            assert value["ok"], value
        return value

    def state(self):
        return self.bus("state")["result"]

    def until(self, predicate, description):
        deadline = time.monotonic() + 15
        while not predicate():
            assert time.monotonic() < deadline, description
            time.sleep(0.05)

    def bridge_records(self):
        path = self.bin / "calls.jsonl"
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def calls(self):
        return [record["argv"] for record in self.bridge_records()]

    def sends(self):
        return self.calls()

    def capture(self, name):
        self.command("snapshot")
        self.command("cells", "0", "0", "160", "55")
        self.command("screenshot", "--out", self.root / f"{name}.png")

    def step(self, number, evidence):
        body = self.report.read_text()
        marker = f"### Step {number}\n"
        start = body.index(marker) + len(marker)
        end = body.find("### Step ", start)
        if end < 0:
            end = len(body)
        body = body[:start] + f"\n- Result: `PASS`\n- Evidence: {evidence}; numbered command JSON, bridge ledger and driver trace under `{self.root}`.\n\n" + body[end:]
        self.report.write_text(body)

    def assert_bridge(self):
        records = self.bridge_records()
        for record in records:
            args = record["argv"]
            assert len(args) == 7 and args[:2] == ["send", "--open-id"], args
            assert args[3] == "--text" and args[5] == "--idempotency-key", args
            assert record["outbox_verified"] and record["outbox"]["key"] == args[6]
            assert record["outbox"]["attempted"]
        assert len({record["message_id"] for record in records}) == len(records)

    def quiet(self):
        self.command("wait", "--stable", "3s", "--rows", "0-0", "--timeout", "8s")

    def no_popup(self):
        value = self.command("effects", "--kind", "terminal_notification")
        assert not value.get("effects"), value

    def finish(self):
        self.command("stop", ok=False)
        checked = subprocess.run([sys.executable, str(REPORT_TOOL), "check", "--doc", self.doc, str(self.report)], cwd=ROOT, capture_output=True, text=True)
        self.report.write_text(self.report.read_text() + "\n## Summary\n\n" + checked.stdout + checked.stderr)
        print(f"{checked.stdout.strip()} — {self.report}", flush=True)
        return checked.returncode

    def fail_remaining(self, reason):
        text = self.report.read_text()
        sections = text.split("### Step ")
        for index in range(1, len(sections)):
            if "- Result: `PENDING`" not in sections[index]:
                continue
            trace = str(Path(self.session["run_dir"]) / "trace.ndjson") if self.session else "none"
            sections[index] = sections[index].replace("- Result: `PENDING`", "- Result: `FAIL`")
            evidence_start = sections[index].index("- Evidence:")
            sections[index] = sections[index][:evidence_start] + (
                f"- Evidence: Acceptance stopped: {reason}\n"
                "- Expected: Complete the documented step and assertions\n"
                f"- Seen: {reason}\n- Trace: `{trace}`\n- Screenshot: none\n"
                "- Effects: Recorded calls and numbered command logs retained in the run folder\n"
                "- Fault guess: `product`\n- Fault evidence: The assertion or command failed; inspect the recorded invocation before attributing a cause\n\n"
            )
        self.report.write_text("### Step ".join(sections))
