#!/usr/bin/env python3
"""Opt-in scratch acceptance. Importing launches nothing; no pytest test functions."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time
import traceback

ROOT = Path(__file__).resolve().parents[2]
DOC = "tests/e2e-tui/feishu-notify.md"
REPORT_TOOL = ROOT / "skills/e2e-test/scripts/e2e_report.py"
TAG = "[waiting_on_you]"


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
        shutil.copyfile(ROOT / "src/messaging/coordinator/tests/waiting_cli_fixture.txt", self.cli)
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
        body = body[:start] + f"\n- Result: `PASS`\n- Evidence: {evidence}; numbered command JSON, CLI ledger and driver trace under `{self.root}`.\n\n" + body[end:]
        self.report.write_text(body)

    def post(self, text):
        return self.bus("send", "--room", "master", "--as", "notify-orch", "--to", "human", "--text", text)

    def assert_bot(self):
        for args in self.sends():
            assert args[:4] == ["im", "+messages-send", "--as", "bot"], args
            assert args.count("--user-id") == 1 and args.count("--text") == 1
            assert "--chat-id" not in args and "--file" not in args and "--attachment" not in args
            assert args[8] == "--idempotency-key" and args[10:] == ["--format", "json"]

    def quiet(self):
        self.command("wait", "--stable", "3s", "--rows", "0-0", "--timeout", "8s")

    def no_popup(self):
        value = self.command("effects", "--kind", "terminal_notification")
        assert not value.get("effects"), value

    def edit_setting(self, hit, text):
        self.command("click", "--hit", hit)
        self.command("press", "ctrl+a", "ctrl+k")
        self.command("type", text)
        self.command("press", "enter")

    def exercise(self):
        self.session = self.command("start", "--size", "160x55", "--graphics", "none", "--run-dir", self.root / "sessions")
        self.report.write_text(self.report.read_text().replace("- Session: `none`", f"- Session: `{self.session['run_dir']}`"))
        self.bus("room", "create", "notify-work")
        work = Path(self.session["data_dir"]).parent / "work"
        self.bus("agent", "add", "--room", "master", "--name", "notify-orch", "--provider", "claude", "--pwd", work, "--orchestrates", "notify-work", "--consent-hooks")
        self.command("wait", "--state", "agents.*.status == idle", "--timeout", "20s")
        self.command("press", "f6")
        self.command("click", "--hit", "settings")
        self.command("wait", "--text", "Feishu notifications (always on)")
        screen = str(self.command("snapshot"))
        assert "ou_ca1b530d792419a044e281f998ce68dc" in screen and "lark-cli path: PATH" in screen
        assert "Developer message notifications" not in screen
        self.capture("settings-default")
        self.command("press", "esc")
        self.command("click", "--hit", "room:notify-work")
        self.command("type", "KEEP DRAFT")
        self.step(1, "Default Feishu always on, recipient/PATH visible, retired toggles absent; settings-default.png captured for visual inspection")

        self.post("Routine progress")
        self.post("[needs_developer_attention] Internal notice only")
        self.quiet()
        assert len(self.calls()) == 1, self.calls()
        assert self.calls()[0][7] == "notify-orch to you: Routine progress\nnotify-orch to you: Internal notice only"
        self.post(TAG + "\nApprove access\nPRIVATE attachment /tmp/secret")
        self.until(lambda: len(self.sends()) == 2, "one underscore-tag BOT DM")
        args = self.sends()[-1]
        assert args[5] == "ou_ca1b530d792419a044e281f998ce68dc" and args[7] == "notify-orch is waiting on you: Approve access", args
        self.quiet()
        assert len(self.sends()) == 2
        self.no_popup()
        self.command("wait", "--text", "KEEP DRAFT")
        self.step(2, "Untagged direct posts notified; underscore --to human produced one exact BOT action-line DM, no private body/attachment/popup; draft preserved")

        long_action = "Full action 界 " * 100
        self.post("[waiting-on-you] " + long_action)
        self.post("Routine progress")
        self.post("WAITING ON YOU: Second update")
        self.until(lambda: len(self.sends()) == 3, "one coalesced burst")
        assert self.sends()[-1][7] == f"notify-orch is waiting on you: {long_action.strip()}\nnotify-orch to you: Routine progress\nnotify-orch is waiting on you: Second update", self.sends()
        self.quiet()
        assert len(self.sends()) == 3
        self.assert_bot()
        self.step(3, "Full long Unicode action and uppercase whole phrase coalesced into one second DM, each formatted line preserved; untagged direct member included without duplicates; all argv explicitly BOT/user-id/plain-text")

        self.command("click", "--hit", "settings")
        self.command("click", "--hit", "feishu-notifications")
        self.command("wait", "--text", "Feishu notifications (always on)")
        assert self.state()["settings"]["feishu_waiting_notifications"] is True
        assert "freeze_notifications" not in self.state()["settings"]
        self.capture("settings-mandatory")
        self.post(TAG + " Mandatory delivery")
        self.until(lambda: len(self.sends()) == 4, "mandatory delivery after toggle attempt")
        self.quiet()
        assert len(self.sends()) == 4
        self.step(4, "UI toggle could not disable delivery; saved preference true and freeze setting absent; mandatory tag delivered; settings-mandatory.png captured for inspection")

        self.edit_setting("feishu-recipient", "ou_fake_recipient")
        self.command("wait", "--text", "Recipient open_id: ou_fake_recipient")
        self.edit_setting("feishu-cli-path", str(self.cli))
        self.command("wait", "--text", "lark-cli path:")
        saved = self.state()["settings"]
        assert saved["feishu_recipient"] == "ou_fake_recipient" and saved["feishu_cli_path"] == str(self.cli), saved
        self.capture("settings-edited")
        self.post("still waiting on you: Configured action")
        self.until(lambda: len(self.sends()) == 5, "configured destination")
        assert self.sends()[-1][5] == "ou_fake_recipient" and self.sends()[-1][7] == "notify-orch is waiting on you: Configured action"
        self.step(5, "UI recipient/CLI override persisted, configured phrase sent to ou_fake_recipient; settings-edited.png captured for inspection")

        (self.bin / "mode").write_text("fail")
        self.post(TAG + " Failure action")
        self.until(lambda: len(self.sends()) == 6, "failed send invocation")
        self.until(lambda: self.state()["settings"]["feishu"]["error"] is not None, "failure status")
        error = self.state()["settings"]["feishu"]["error"]
        assert "exit code Some(7)" in error, error
        self.quiet()
        assert len(self.sends()) == 6 and self.command("status")["bus_pid"] == self.session["bus_pid"]
        self.no_popup()
        self.assert_bot()
        log_root = Path(self.session["data_dir"]) / "herdr-config"
        logs = "\n".join(p.read_text(errors="replace") for p in log_root.rglob("*.log"))
        assert "bus.feishu.failed" in logs and "exit code Some(7)" in logs, str(log_root)
        (self.root / "calls.json").write_text(json.dumps(self.calls(), indent=2))
        (self.root / "failure-warning.txt").write_text("\n".join(line for line in logs.splitlines() if "bus.feishu.failed" in line))
        self.step(6, "Exit-7 failure logged bus.feishu.failed with error; one attempt, responsive unchanged PID, no retry or terminal popup; failure-warning.txt and calls.json retained")

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
