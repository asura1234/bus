#!/usr/bin/env python3
"""Opt-in scratch acceptance. Importing launches nothing; no pytest test functions."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import time
import traceback

ROOT = Path(__file__).resolve().parents[2]
DOC = "tests/e2e-tui/feishu-notify.md"
REPORT_TOOL = ROOT / "skills/e2e-test/scripts/e2e_report.py"
TAG = "[needs_developer_attention]"


class Run:
    def __init__(self, binary, name):
        self.binary = Path(binary).resolve()
        self.name = "feishu-" + hashlib.sha256(name.encode()).hexdigest()[:8]
        self.env = {k: v for k, v in os.environ.items() if not k.startswith(("BUS_", "HERDR_", "CLAUDE"))}
        commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
        self.report = Path(subprocess.check_output([
            sys.executable, str(REPORT_TOOL), "init", "--doc", DOC, "--name", name,
            "--runs-root", str(ROOT / "temp/feishu-notify"), "--commit", commit,
            "--platform", "macOS scratch driver 160x55, graphics none",
        ], cwd=ROOT, text=True).strip())
        self.root = self.report.parent
        self.bin = self.root / "fake-bin"
        self.bin.mkdir()
        self.cli = self.bin / "lark-cli"
        shutil.copyfile(ROOT / "src/messaging/feishu/tests/lark_fixture.txt", self.cli)
        self.cli.chmod(0o700)
        self.env["PATH"] = str(self.bin) + ":/usr/bin:/bin:/usr/sbin:/sbin"
        assert shutil.which("lark-cli", path=self.env["PATH"]) == str(self.cli)
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

    def calls(self):
        path = self.bin / "calls.jsonl"
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def sends(self):
        return [call for call in self.calls() if call["args"][0] == "im"]

    def consumed(self, minimum):
        path = Path(self.session["data_dir"]) / "state.json"
        state = json.loads(path.read_text())
        state = state.get("state", state)
        return len(state.get("attention_delivery", {}).get("handled", [])) >= minimum

    def capture(self, name):
        self.command("snapshot")
        self.command("cells", "1", "1", "160", "55")
        self.command("screenshot", "--out", self.root / f"{name}.png")

    def step(self, number, evidence):
        body = self.report.read_text()
        marker = f"### Step {number}\n"
        start = body.index(marker) + len(marker)
        end = body.find("### Step ", start)
        if end < 0:
            end = len(body)
        body = body[:start] + f"\n- Result: `PASS`\n- Evidence: {evidence}; numbered command JSON, CLI ledger and driver trace under `{self.root}`.\n\n" + body[end:]
        self.report.write_text(body)

    def post(self, text):
        return self.bus("send", "--room", "master", "--as", "notify-orch", "--to", "human", "--text", text)

    def assert_self_only(self):
        for call in self.sends():
            args, body = call["args"], call["body"]
            assert args.count("--user-id") == 1
            assert args[args.index("--user-id") + 1] == call["self"] == "ou_self_a"
            assert args[args.index("--as") + 1] == "user" and "--chat-id" not in args
            assert body.startswith("[Bus]")
            assert all(field in body for field in ["Machine: ", "Session: ", "Room: ", "Agent: ", "notify-orch"])

    def exercise(self):
        self.session = self.command("start", "--size", "160x55", "--graphics", "none", "--run-dir", self.root / "sessions")
        self.report.write_text(self.report.read_text().replace("- Session: `none`", f"- Session: `{self.session['run_dir']}`"))
        self.bus("room", "create", "notify-work")
        work = Path(self.session["data_dir"]).parent / "work"
        self.bus("agent", "add", "--room", "master", "--name", "notify-orch", "--provider", "claude", "--pwd", work, "--orchestrates", "notify-work", "--consent-hooks")
        self.command("press", "f6")
        self.command("click", "--hit", "settings")
        self.command("wait", "--text", "[ ] Feishu notifications")
        (self.bin / "mode").write_text("missing")
        self.command("click", "--hit", "feishu-notifications")
        self.command("wait", "--text", "no authenticated user")
        assert not self.state()["settings"]["feishu_notifications"]
        self.cli.rename(self.bin / "lark-disabled")
        refused = self.bus("settings", "feishu-notifications", "--on", ok=False)
        assert not refused["ok"] and "not installed" in refused["error"]["message"]
        self.cli = self.bin / "lark-cli"
        (self.bin / "lark-disabled").rename(self.cli)
        self.capture("auth-refusal")
        self.step(1, "Default checkbox off; missing auth visible and persisted off; missing binary enable refused on closed PATH")

        (self.bin / "mode").write_text("ready")
        self.command("click", "--hit", "feishu-notifications")
        self.command("wait", "--text", "[x] Feishu notifications")
        self.capture("settings-on")
        self.bus("settings", "feishu-notifications", "--off")
        # Public CLI without --dev, against only this scratch directory.
        env = dict(self.env, BUS_DATA_DIR=self.session["data_dir"])
        enabled = subprocess.run([str(self.binary), "settings", "feishu-notifications", "--on"], env=env, capture_output=True, text=True, timeout=20)
        (self.root / "public-enable.json").write_text(enabled.stdout)
        assert enabled.returncode == 0 and json.loads(enabled.stdout)["ok"]
        self.bus("diagnostics")
        self.command("wait", "--text", "not installed", "--gone")
        for field in ["recipient", "user_id", "chat_id", "group", "feishu_recipient"]:
            with socket.socket(socket.AF_UNIX) as connection:
                connection.connect(str(Path(self.session["data_dir"]) / "control.sock"))
                connection.sendall((json.dumps({"id": "destination-" + field, "method": "settings.feishu_notifications", "params": {"on": True, field: "ou_other"}}) + "\n").encode())
                response = json.loads(connection.makefile().readline())
            assert not response["ok"] and response["error"]["code"] == "invalid_params"
        assert not self.sends()
        self.step(2, "UI enable and public CLI off/on in one process succeeded; every destination input rejected before CLI; settings-on.png")

        self.command("press", "esc")
        self.command("click", "--hit", "room:MASTER")
        # Existing error toasts last fifteen seconds even after the status clears.
        self.command("wait", "--text", "not installed", "--gone", "--timeout", "20s")
        self.post("Routine progress without developer action")
        self.post(TAG + " Choose deployment target")
        self.until(lambda: len(self.sends()) == 1, "tagged send")
        self.command("wait", "--text", "[Needs developer]")
        screen = "\n".join(self.command("snapshot")["rows"])
        assert TAG not in screen
        self.assert_self_only()
        self.capture("master-badge")
        self.step(3, "Only tagged orchestrator message forwarded with all Bus context and self recipient; MASTER badge replaces raw tag; master-badge.png")

        self.post(TAG + " Choose deployment target")
        self.until(lambda: self.consumed(2), "duplicate consumed")
        assert len(self.sends()) == 1
        self.bus("settings", "feishu-notifications", "--off")
        self.post(TAG + " Disabled history")
        self.bus("settings", "feishu-notifications", "--on")
        assert len(self.sends()) == 1
        self.step(4, "Repeated content consumed without send; off suppressed messages; re-enable skipped disabled history")

        self.bus("send", "--room", "master", "--to", "notify-orch", "--text", "fake:raw-reply " + TAG + " Final decision")
        self.until(lambda: len(self.sends()) == 2, "tagged final")
        self.bus("send", "--room", "master", "--to", "notify-orch", "--text", "fake:dialog 20")
        self.until(lambda: len(self.sends()) == 3, "mechanical blocked notice")
        self.command("wait", "--state", "agents.*.dialog == true")
        assert "Blocked, needs help" in self.sends()[-1]["body"]
        self.command("wait", "--stable", "1500ms", "--rows", "1-2")
        assert len(self.sends()) == 3
        self.assert_self_only()
        assert self.command("status")["bus_pid"] == self.session["bus_pid"]
        self.capture("mechanical")
        (self.root / "calls.json").write_text(json.dumps(self.calls(), indent=2))
        self.step(5, "Tagged provider final and one mechanical Bus dialog notice forwarded; stabilized duplicate sent nothing; all three sends self-only and attributed to Bus; PID unchanged")

        for index in range(5):
            self.post(TAG + f" Rate-limit decision {index}")
        self.until(lambda: len(self.sends()) == 6, "six attempts allowed")
        self.command("wait", "--stable", "1500ms", "--rows", "1-2")
        assert len(self.sends()) == 6
        self.assert_self_only()
        (self.root / "calls.json").write_text(json.dumps(self.calls(), indent=2))
        self.step(6, "Burst capped at six total sends in the rolling minute; additional tagged sources remain in durable MASTER history; every attempt used self")

    def finish(self):
        self.command("stop", ok=False)
        checked = subprocess.run([sys.executable, str(REPORT_TOOL), "check", "--doc", DOC, str(self.report)], cwd=ROOT, capture_output=True, text=True)
        self.report.write_text(self.report.read_text() + "\n## Summary\n\n" + checked.stdout + checked.stderr)
        print(f"{checked.stdout.strip()} — {self.report}", flush=True)
        return checked.returncode

    def fail_remaining(self, reason):
        """Retain a failed attempt as a complete RED report, never a quiet retry."""
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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--run-name", required=True)
    args = parser.parse_args()
    run = Run(args.binary, args.run_name)
    try:
        run.exercise()
    except Exception as error:
        run.fail_remaining(str(error).replace("\n", " "))
        traceback.print_exc()
    finally:
        result = run.finish()
    return result if "Verdict: GREEN" in run.report.read_text() else 1


if __name__ == "__main__":
    raise SystemExit(main())
