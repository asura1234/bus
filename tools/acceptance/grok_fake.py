"""Isolated Grok/Claude fake integration. No credentials or real providers.

Uses F's fake CLIs and the disposable acceptance Bus lifecycle, with a closed
PATH, empty home and explicit binary. The synthetic completion schema proves
Bus plumbing only; signed-in native Grok acceptance remains pending.
"""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import struct
import signal
import subprocess
import termios
import tempfile
import time
import uuid

from tools.acceptance.harness_test import Bus, CliError

ROOT = Path(__file__).resolve().parents[2]


def poll(read, accept, timeout=45):
    deadline = time.monotonic() + timeout
    while True:
        value = read()
        if accept(value):
            return value
        if time.monotonic() >= deadline:
            raise AssertionError(f"condition timed out: {str(value)[:1800]}")
        time.sleep(0.1)


class Run:
    def __init__(self, binary):
        if any(k.startswith(("BUS_", "HERDR_")) for k in os.environ):
            raise ValueError("scrub BUS_*/HERDR_* before running fake acceptance")
        binary = Path(binary)
        if not binary.is_absolute() or not binary.is_file():
            raise ValueError("an explicit absolute dev binary is required")
        if binary.parts[-3:] == ("target", "debug", "bus"):
            raise ValueError("do not use the main checkout's binary")
        self.binary = binary.resolve()
        self.artifacts = ROOT / "temp/grok-provider/fake-runs" / (time.strftime("%Y%m%d-%H%M%S") + "-" + uuid.uuid4().hex[:8])
        self.artifacts.mkdir(parents=True, mode=0o700)
        # macOS Unix-domain sockets have a short pathname limit.
        self.root = Path(tempfile.mkdtemp(prefix="bg-", dir="/private/tmp"))
        for name in ("home", "work", "bin", "data"):
            (self.root / name).mkdir(parents=True, exist_ok=False, mode=0o700)
        self.root.chmod(0o700)
        for name in ("grok", "claude", "codex", "cursor-agent", "open", "osascript", "afplay", "terminal-notifier"):
            (self.root / "bin" / name).symlink_to(self.binary)
        shim = self.root / "bin/lark-cli"
        shim.write_text("#!/bin/sh\nexit 0\n")
        shim.chmod(0o700)
        self.env = {
            "PATH": str(self.root / "bin") + ":/usr/bin:/bin:/usr/sbin:/sbin",
            "HOME": str(self.root / "home"), "SHELL": "/bin/sh", "TERM": "xterm-256color",
            "GROK_HOME": str(self.root / "home/.grok"),
            "XDG_CONFIG_HOME": str(self.root / "home/config"),
            "XDG_STATE_HOME": str(self.root / "home/state"),
            "XDG_RUNTIME_DIR": str(self.root / "home/runtime"),
            "BUS_DATA_DIR": str(self.root / "data"),
            "BUS_TUI_FAKE_AGENT": "1", "BUS_TUI_OWNER": uuid.uuid4().hex,
            "BUS_TUI_EFFECTS": str(self.root / "effects.ndjson"),
        }
        subprocess.run(["nice", "-n", "19", "/usr/bin/git", "init", "--quiet", str(self.root / "work")], env=self.env, check=True)
        self.bus = Bus(self.binary, self.artifacts, self.env, workspace=self.root / "work")
        self.report = {
            "head": subprocess.check_output(["nice", "-n", "19", "git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
            "binary_sha256": hashlib.sha256(self.binary.read_bytes()).hexdigest(),
            "provenance": "synthetic", "native": "PENDING_AUTH", "cases": {},
            "scratch_root": str(self.root),
        }

    def cli(self, *args):
        result = self.bus.cli(*map(str, args))
        with (self.artifacts / "trace.ndjson").open("a") as trace:
            trace.write(json.dumps({"at": time.time(), "argv": list(map(str, args)), "result": result}, ensure_ascii=False) + "\n")
        return result

    def start(self):
        if self.bus.pump_thread is not None and self.bus.child.poll() is not None:
            self.bus.pump_thread.join(timeout=2)
        self.bus.start()
        # The whole first-message bootstrap must remain visibly attributable;
        # a synthetic paste-chip count is deliberately insufficient evidence.
        fcntl.ioctl(self.bus.master, termios.TIOCSWINSZ, struct.pack("HHHH", 400, 400, 0, 0))
        self.cli("settings", "feishu-notifications", "--off")

    def ready(self, agent):
        return poll(lambda: next(a for a in self.cli("diagnostics")["agents"] if a["agent_id"] == agent),
                    lambda a: a["status"] == "idle" and a["reason"] is None)

    def add(self, room, name, provider="grok", orchestrates=None):
        args = ["agent", "add", "--room", room, "--name", name, "--provider", provider,
                "--pwd", self.root / "work", "--consent-hooks"]
        if orchestrates:
            args += ["--orchestrates", orchestrates]
        agent = self.cli(*args)["agent_id"]
        self.ready(agent)
        return agent

    def send(self, room, agent, text, author=None, queue=False):
        args = ["send", "--room", room, "--to", agent, "--text", text]
        if author is not None:
            args += ["--as", author]
        if queue:
            args += ["--queue"]
        return self.cli(*args)["message_id"]

    def dialog_action(self, *args):
        try:
            return self.cli(*args)
        except CliError as error:
            if "Dialog answer outcome is uncertain" not in str(error):
                raise
            # Never repeat an uncertain keypress. Settlement below supplies
            # evidence that this scripted answer was actually accepted.
            return {"outcome": "uncertain", "error": str(error), "retried": False}

    def settle(self, message, success=True):
        status = poll(lambda: self.cli("message", "status", message), lambda s: s["complete"])
        if success:
            assert all(r["stage"] == "replied" for r in status["requests"]), status
            assert all(not r.get("uncertain_outcome") for r in status["requests"]), status
        return status

    def case(self, name, call):
        try:
            self.report["cases"][name] = {"result": "PASS", "evidence": call()}
        except Exception as error:
            self.report["cases"][name] = {"result": "FAIL", "error": f"{type(error).__name__}: {error}"}
            raise
        finally:
            self.save()

    def save(self):
        (self.artifacts / "report.json").write_text(json.dumps(self.report, ensure_ascii=False, indent=2))

    def worker_cases(self):
        self.room = self.cli("room", "create", "grok-fake")["room_id"]
        self.worker = self.add(self.room, "grok-worker")

        def round_trip():
            message = self.send(self.room, self.worker, "fake:raw-reply WORKER_OK")
            status = self.settle(message)
            assert status["requests"][0]["reply"]["text"] == "WORKER_OK", status
            return status
        self.case("worker-round-trip", round_trip)

        def queued():
            ids = [self.send(self.room, self.worker, f"fake:raw-reply QUEUED_{n}", queue=True) for n in range(3)]
            rows = [self.settle(i) for i in ids]
            assert [r["requests"][0]["reply"]["text"] for r in rows] == [f"QUEUED_{n}" for n in range(3)]
            assert len({r["requests"][0]["turn_id"] for r in rows}) == 3
            return rows
        self.case("queued", queued)

        def boundaries():
            rows = []
            for prompt in ("fake:gate fake:raw-reply GATE_OK", "fake:no-final-stop fake:raw-reply NO_STOP_OK", "fake:long"):
                status = self.settle(self.send(self.room, self.worker, prompt))
                rows.append(status)
            assert rows[0]["requests"][0]["reply"]["text"] == "GATE_OK"
            assert rows[1]["requests"][0]["reply"]["text"] == "NO_STOP_OK"
            assert rows[2]["requests"][0]["reply"]["text"] == "界" * 40000
            return {"gate": rows[0], "no_stop": rows[1], "long_chars": 40000}
        self.case("committed-boundaries", boundaries)

        def steering():
            first = self.send(self.room, self.worker, "fake:work 2 fake:raw-reply STEERING_OK")
            poll(lambda: self.cli("message", "status", first),
                 lambda s: s["requests"][0]["stage"] == "delivered" and s["requests"][0]["start_bound"])
            second = self.send(self.room, self.worker, "STEERING_CORRECTION")
            rows = [self.settle(first), self.settle(second)]
            assert rows[0]["requests"][0]["turn_id"] == rows[1]["requests"][0]["turn_id"]
            assert rows[0]["requests"][0]["reply"] == rows[1]["requests"][0]["reply"]
            assert "STEERING_CORRECTION" in rows[0]["requests"][0]["reply"]["text"]
            return rows
        self.case("steering", steering)

        def dialogs():
            records = []
            for prompt, skip in (("fake:question", False), ("fake:free-text", True)):
                message = self.send(self.room, self.worker, prompt)
                found = poll(lambda: self.cli("agent", "dialog", self.worker), lambda r: r.get("dialog"))
                if skip:
                    outcome = self.dialog_action("agent", "answer", self.worker, "--skip", "--fingerprint", found["fingerprint"])
                else:
                    outcome = self.dialog_action("agent", "choose", self.worker, "--option", 1, "--fingerprint", found["fingerprint"])
                records.append({"observed": found, "answer": outcome, "settled": self.settle(message)})
            return records
        self.case("question-and-dismissal", dialogs)

        def errors():
            rows = []
            for prompt in ("fake:api-error", "fake:gate-cancel"):
                status = self.settle(self.send(self.room, self.worker, prompt), success=False)
                assert all(r["reply"] is None and r["stage"] != "replied" for r in status["requests"]), status
                rows.append(status)
            return rows
        self.case("failure-and-cancellation", errors)

    def history_reply(self, room, agent, token):
        history = self.cli("history", "--room", room)
        return [r for m in history["messages"] for r in (m.get("delivery") or {}).get("requests", [])
                if r["agent_id"] == agent and token in ((r.get("reply") or {}).get("text") or "")]

    def unread(self, room):
        saved = json.loads((self.root / "data/state.json").read_text())
        return saved["state"]["rooms"][str(room)]["unread_count"]

    def stable_publication(self, room, agent, token):
        rows = poll(lambda: self.history_reply(room, agent, token), lambda rows: len(rows) == 1)
        count = self.unread(room)
        deadline = time.monotonic() + 2.5
        reads = 0
        while time.monotonic() < deadline:
            assert self.unread(room) == count, "publication counter changed on repeated reads"
            assert len(self.history_reply(room, agent, token)) == 1, "duplicate/missing orchestrator reply"
            reads += 1
            time.sleep(0.1)
        return {"reply": rows[0], "unread": count, "stable_reads": reads}

    def joint_cases(self):
        # Orchestrator decisions are scripted fixtures. The actual Bus wake,
        # prompt delivery, callbacks and final routing run without manual input.
        self.claude_orch = self.add("master", "claude-orch", "claude", self.room)
        self.joint = self.cli("room", "create", "joint-wake")["room_id"]
        self.orch = self.add("master", "grok-orch", orchestrates=self.joint)
        self.claude = self.add(self.joint, "claude-worker", "claude")
        self.cli("room", "focus", "master")

        def grok_worker_claude_orchestrator():
            message = self.send(self.room, self.worker, "fake:raw-reply GROK_TO_CLAUDE", author=self.claude_orch)
            self.settle(message)
            return self.stable_publication(self.room, self.claude_orch, "GROK_TO_CLAUDE")
        self.case("grok-worker-claude-orchestrator", grok_worker_claude_orchestrator)

        def master_and_wake():
            self.master_one = self.send("master", self.orch, "fake:raw-reply MASTER_ONE")
            one = self.settle(self.master_one)
            assert one["requests"][0]["reply"]["text"] == "MASTER_ONE", one
            job = self.send(self.joint, self.claude, "fake:work 3 fake:raw-reply WORKER_LATE_RESULT", author=self.orch)
            self.master_two = self.send("master", self.orch, "fake:raw-reply MASTER_TWO")
            two = self.settle(self.master_two)
            assert two["requests"][0]["reply"]["text"] == "MASTER_TWO", two
            self.settle(job)
            wake = self.stable_publication(self.joint, self.orch, "WORKER_LATE_RESULT")
            assert self.cli("message", "status", self.master_one)["requests"][0]["reply"] == one["requests"][0]["reply"]
            assert self.cli("message", "status", self.master_two)["requests"][0]["reply"] == two["requests"][0]["reply"]
            self.master_replies = [one["requests"][0]["reply"], two["requests"][0]["reply"]]
            return {"master_one": one, "master_two": two, "worker": job, "wake": wake}
        self.case("grok-orchestrator-wake-and-master-ownership", master_and_wake)

        def blocker():
            self.peer = self.add(self.joint, "grok-peer")
            job = self.send(self.joint, self.peer, "fake:question", author=self.orch)
            found = poll(lambda: self.cli("agent", "dialog", self.peer), lambda r: r.get("dialog"))
            notice = poll(lambda: self.cli("agent", "read", self.orch, "--source", "recent", "--lines", 200),
                          lambda r: "Blocked, needs help to continue." in r.get("text", ""))
            outcome = self.dialog_action("agent", "choose", self.peer, "--option", 1, "--fingerprint", found["fingerprint"])
            return {"blocker_wake": notice, "scripted_orchestrator_answer": outcome, "settled": self.settle(job)}
        self.case("worker-blocker-wake-and-answer", blocker)

    def resume_cases(self):
        agents = [self.worker, self.orch, self.claude, self.claude_orch, self.peer]

        def cold_resume():
            before = {a: self.ready(a)["identity"] for a in agents}
            stopped = self.bus.stop()
            self.start()
            after = {a: self.ready(a)["identity"] for a in agents}
            for agent in agents:
                assert before[agent]["session_id"] == after[agent]["session_id"], (before, after)
                assert before[agent]["terminal_id"] != after[agent]["terminal_id"], (before, after)
            message = self.send("master", self.orch, "fake:raw-reply RESUMED_MASTER")
            assert self.settle(message)["requests"][0]["reply"]["text"] == "RESUMED_MASTER"
            for message, reply in zip((self.master_one, self.master_two), self.master_replies):
                assert self.cli("message", "status", message)["requests"][0]["reply"] == reply
            return {"before": before, "after": after, "stop": stopped, "resumed_message": message}
        self.case("cold-resume-and-master-history", cold_resume)

        def restart_before_wake():
            job = self.send(self.joint, self.claude, "fake:work 2 fake:raw-reply RESTART_WAKE", author=self.orch)
            poll(lambda: self.cli("message", "status", job), lambda r: r["requests"][0]["start_bound"])
            pid = self.bus.child.pid
            os.kill(pid, signal.SIGSTOP)
            try:
                def durable_final():
                    for path in (self.root / "data/callbacks").glob("*/event-*.json"):
                        record = json.loads(path.read_text())
                        if "RESTART_WAKE" in str(record["value"].get("last_assistant_message", "")):
                            return str(path)
                    return None
                final = poll(durable_final, bool)
            except BaseException:
                os.kill(pid, signal.SIGCONT)
                raise
            # Stop/restart before allowing the coordinator another poll. The
            # final spool is the durable boundary, not an invented replay.
            os.kill(pid, signal.SIGKILL)
            self.bus.child.wait(timeout=5)
            self.bus.stop()
            self.start()
            self.ready(self.orch)
            self.settle(job)
            wake = self.stable_publication(self.joint, self.orch, "RESTART_WAKE")
            for message, reply in zip((self.master_one, self.master_two), self.master_replies):
                assert self.cli("message", "status", message)["requests"][0]["reply"] == reply
            return {"durable_final_before_restart": final, "job": job, "wake": wake}
        self.case("restart-before-worker-wake", restart_before_wake)

    def execute(self):
        try:
            self.start()
            self.worker_cases()
            self.joint_cases()
            self.resume_cases()
        finally:
            self.report["cleanup"] = self.bus.stop()
            self.save()


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    args = parser.parse_args(argv)
    run = Run(args.binary)
    try:
        run.execute()
    except Exception as error:
        run.report["error"] = f"{type(error).__name__}: {error}"
        run.save()
        print(json.dumps({"result": "FAIL", "error": str(error), "report": str(run.artifacts / "report.json")}))
        return 1
    print(json.dumps({"result": "PASS", "report": str(run.artifacts / "report.json")}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
