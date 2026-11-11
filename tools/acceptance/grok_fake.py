"""Isolated Grok/Claude fake integration. No credentials or real providers.

Uses synthetic fake CLIs and the disposable acceptance Bus lifecycle, with a closed
PATH, empty home and explicit binary. The synthetic completion schema proves
Bus plumbing only; signed-in native Grok acceptance remains pending.
"""
import argparse
import fcntl
import hashlib
import json
import os
import re
from pathlib import Path
import struct
import signal
import subprocess
import termios
import tempfile
import time
import uuid

from tools.acceptance.harness_test import Bus, CliError, observation_race

ROOT = Path(__file__).resolve().parents[2]


def poll(read, accept, timeout=45):
    deadline = time.monotonic() + timeout
    while True:
        try:
            value = read()
        except CliError as error:
            if not observation_race(error) or time.monotonic() >= deadline:
                raise
            time.sleep(0.1)
            continue
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
            "HOME": str(self.root / "home"), "SHELL": "/bin/zsh", "TERM": "xterm-256color",
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
        # Use the same realistic viewport as live acceptance. The old 400x400
        # fixture continuously repainted faster than a coherent Grid read.
        # Per-turn transcripts below retain exact payload/output attribution.
        fcntl.ioctl(self.bus.master, termios.TIOCSWINSZ, struct.pack("HHHH", 45, 160, 0, 0))

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

    def driver_action(self, agent, observation, *inputs):
        deadline = time.monotonic() + 5
        rejected = 0
        while True:
            try:
                result = self.cli("agent", "press", agent, "--terminal-id", observation["terminal_id"], *inputs)
                break
            except CliError as error:
                if not observation_race(error) or time.monotonic() >= deadline:
                    raise
                fresh = poll(lambda: self.cli("agent", "observe", agent), bool, timeout=5)
                assert all(fresh[key] == observation[key] for key in ("terminal_id", "text")), \
                    "menu or terminal changed after rejection; inspect before continuing"
                rejected += 1
        assert result.get("outcome") in ("sent", "selected") and not result.get("selection_pending"), \
            f"driver input is pending or uncertain; inspect it without replaying: {result}"
        return {**result, "pre_input_reobservations": rejected}

    def settle(self, message, success=True, reply=None):
        status = poll(lambda: self.cli("message", "status", message), lambda s: s["complete"])
        if success:
            assert status["requests"], status
            for request in status["requests"]:
                assert not request.get("uncertain_outcome") and request.get("reason") is None, status
                assert request.get("start_bound"), status
                if reply is None:
                    assert request["stage"] == "ended" and request.get("turn_ended"), status
                    assert request.get("reply") is None, status
                else:
                    assert request["stage"] == "replied", status
                    assert (request.get("reply") or {}).get("text") == reply, status
        return status

    def native_answer(self, status, expected):
        for request in status["requests"]:
            transcript = self.root / "work/.grok/bus-fake" / request["session_id"] / "chat_history.jsonl"
            records = [json.loads(line) for line in transcript.read_text().splitlines()]
            answers = [r["content"] for r in records if r["promptId"] == request["turn_id"] and r["role"] == "agent"]
            assert answers == [expected], {"request": request, "answers": answers}
        return status

    def explicit_answer(self, message, agent, token):
        poll(lambda: self.cli("message", "status", message),
             lambda s: s["requests"][0]["start_bound"])
        receipt = self.cli("send", "--reply-to", message, "--as", agent, "--text", token)
        assert receipt["stage"] == "replied", receipt
        status = self.settle(message, reply=token)
        self.ready(agent)
        assert self.settle(message, reply=token)["requests"][0]["reply"] == status["requests"][0]["reply"]
        return status

    def result_message(self, room, worker, requester, token):
        # A scripted worker-authored normal send, separate from reply settlement.
        self.ready(requester)
        message = self.send(room, requester, "fake:raw-reply " + token, author=worker)
        status = self.settle(message)
        self.ready(requester)
        if requester == self.claude_orch:
            output = self.cli("agent", "read", requester, "--source", "recent", "--lines", 200)["text"]
            assert re.search(r"(?m)^\s*⏺ " + re.escape(token) + r"\s*$", output), output
        else:
            self.native_answer(status, token)
        return self.stable_publication(room, requester, token)

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
            return self.native_answer(self.settle(message), "WORKER_OK")
        self.case("worker-round-trip", round_trip)

        def queued():
            ids = [self.send(self.room, self.worker, f"fake:raw-reply QUEUED_{n}", queue=True) for n in range(3)]
            rows = [self.settle(i) for i in ids]
            for n, status in enumerate(rows):
                self.native_answer(status, f"QUEUED_{n}")
            assert len({r["requests"][0]["turn_id"] for r in rows}) == 3
            return rows
        self.case("queued", queued)

        def boundaries():
            rows = []
            for prompt in ("fake:gate fake:raw-reply GATE_OK", "fake:no-final-stop fake:raw-reply NO_STOP_OK", "fake:long"):
                status = self.settle(self.send(self.room, self.worker, prompt))
                rows.append(status)
            for status, text in zip(rows, ("GATE_OK", "NO_STOP_OK", "界" * 40000)):
                self.native_answer(status, text)
            return {"gate": rows[0], "no_stop": rows[1], "long_chars": 40000}
        self.case("committed-boundaries", boundaries)

        def steering():
            first = self.send(self.room, self.worker, "fake:work 8 fake:raw-reply STEERING_OK")
            poll(lambda: self.cli("message", "status", first),
                 lambda s: s["requests"][0]["stage"] == "delivered" and s["requests"][0]["start_bound"])
            second = self.send(self.room, self.worker, "fake:raw-reply STEERING_CORRECTION")
            pending = self.cli("message", "status", second)["requests"][0]
            assert pending["stage"] == "queued" and pending["group"] is None, pending
            rows = [self.settle(first), self.settle(second)]
            assert rows[0]["requests"][0]["turn_id"] != rows[1]["requests"][0]["turn_id"]
            self.native_answer(rows[0], "STEERING_OK")
            self.native_answer(rows[1], "STEERING_CORRECTION")
            return rows
        self.case("steering", steering)

        def dialogs():
            records = []
            for prompt, skip in (("fake:question", False), ("fake:free-text", True)):
                message = self.send(self.room, self.worker, prompt)
                found = poll(lambda: self.cli("agent", "observe", self.worker),
                             lambda r: "◆ Choose an answer" in r["text"])
                if skip:
                    assert "Enter:submit" in found["text"], found
                    back = self.driver_action(self.worker, found, "--key", "esc")
                    chooser = poll(lambda: self.cli("agent", "observe", self.worker),
                                   lambda r: "Shift+x:dismiss" in r["text"])
                    outcome = {"back": back, "dismiss": self.driver_action(
                        self.worker, chooser, "--key", "shift+x")}
                else:
                    assert "1/2:select" in found["text"], found
                    outcome = self.driver_action(self.worker, found, "--key", "1")
                records.append({"observed": found, "answer": outcome, "settled": self.settle(message)})
            return records
        self.case("question-and-dismissal", dialogs)

        def errors():
            message = self.send(self.room, self.worker, "fake:api-error")
            # Transient API failures are held for native retry, not settled as
            # successful native finals or handed back to the sender for replay.
            hold = poll(lambda: json.loads((self.root / "data/state.json").read_text())
                        ["state"]["agents"][str(self.worker)].get("provider_retry"), bool)
            failed = self.cli("message", "status", message)
            assert not failed["complete"] and all(r["reply"] is None for r in failed["requests"]), failed
            assert hold["lead"] == failed["requests"][0]["request_id"] and hold["attempts"] >= 1, hold
            failed_agent = self.worker
            self.cli("agent", "delete", failed_agent, "--confirm")
            poll(lambda: self.cli("state"), lambda s: all(a["id"] != failed_agent for a in s["agents"]))
            removed = self.settle(message, success=False)
            assert all(r["reply"] is None and r["reason"] == "agent_deleted" for r in removed["requests"])
            self.worker = self.add(self.room, "grok-worker")
            cancelled = self.settle(self.send(self.room, self.worker, "fake:gate-cancel"), success=False)
            assert all(r["reply"] is None and r["stage"] == "abandoned" and r["reason"]
                       for r in cancelled["requests"]), cancelled
            return {"retry_hold": hold, "failed": failed, "removed": removed, "cancelled": cancelled}
        self.case("failure-and-cancellation", errors)

    def history_result(self, room, agent, token):
        history = self.cli("history", "--room", room)
        return [m for m in history["messages"] if agent in m["prompt"]["recipient_ids"]
                and token in m["prompt"]["text"] and isinstance(m["prompt"]["author"], dict)]

    def unread(self, room):
        saved = json.loads((self.root / "data/state.json").read_text())
        return saved["state"]["rooms"][str(room)]["unread_count"]

    def stable_publication(self, room, agent, token):
        rows = poll(lambda: self.history_result(room, agent, token), lambda rows: len(rows) == 1)
        count = self.unread(room)
        deadline = time.monotonic() + 2.5
        reads = 0
        while time.monotonic() < deadline:
            assert self.unread(room) == count, "publication counter changed on repeated reads"
            assert len(self.history_result(room, agent, token)) == 1, "duplicate/missing authored result"
            reads += 1
            time.sleep(0.1)
        requests = rows[0]["delivery"]["requests"]
        assert len(requests) == 1 and requests[0]["stage"] == "ended" and requests[0]["reply"] is None
        return {"message": rows[0], "unread": count, "stable_reads": reads}

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
            self.native_answer(self.settle(message), "GROK_TO_CLAUDE")
            return self.result_message(self.room, self.worker, self.claude_orch, "GROK_TO_CLAUDE")
        self.case("grok-worker-claude-orchestrator", grok_worker_claude_orchestrator)

        def master_and_wake():
            self.master_one = self.send("master", self.orch, "fake:work 3 fake:raw-reply MASTER_ONE")
            one = self.explicit_answer(self.master_one, self.orch, "MASTER_ONE")
            self.master_two = self.send("master", self.orch, "fake:work 3 fake:raw-reply MASTER_TWO")
            two = self.explicit_answer(self.master_two, self.orch, "MASTER_TWO")
            before = self.cli("history", "--room", self.joint)["messages"]
            transcript = self.root / "work/.grok/bus-fake" / two["requests"][0]["session_id"] / "chat_history.jsonl"
            before_turns = transcript.read_text()
            job = self.send(self.joint, self.claude, "fake:work 3 fake:raw-reply WORKER_LATE_RESULT", author=self.orch)
            self.explicit_answer(job, self.claude, "WORKER_LATE_RESULT")
            after = self.cli("history", "--room", self.joint)["messages"]
            assert len(after) == len(before) + 1, "reply settlement synthesized a wake/publication"
            assert self.ready(self.orch)["status"] == "idle"
            assert transcript.read_text() == before_turns, "explicit reply started a requester turn"
            wake = self.result_message(self.joint, self.claude, self.orch, "WORKER_LATE_RESULT")
            assert self.cli("message", "status", self.master_one)["requests"][0]["reply"] == one["requests"][0]["reply"]
            assert self.cli("message", "status", self.master_two)["requests"][0]["reply"] == two["requests"][0]["reply"]
            self.master_replies = [one["requests"][0]["reply"], two["requests"][0]["reply"]]
            return {"master_one": one, "master_two": two, "worker": job, "wake": wake}
        self.case("grok-orchestrator-wake-and-master-ownership", master_and_wake)

        def blocker():
            self.peer = self.add(self.joint, "grok-peer")
            job = self.send(self.joint, self.peer, "fake:question fake:raw-reply BLOCKER_RESOLVED", author=self.orch)
            found = poll(lambda: self.cli("agent", "observe", self.peer),
                         lambda r: "◆ Choose an answer" in r["text"] and "1/2:select" in r["text"])
            notice = poll(lambda: self.cli("agent", "read", self.orch, "--source", "recent", "--lines", 200),
                          lambda r: "Blocked, needs help to continue." in r.get("text", ""))
            outcome = self.driver_action(self.peer, found, "--key", "1")
            settled = self.settle(job)
            self.native_answer(settled, "BLOCKER_RESOLVED")
            resolved = self.result_message(self.joint, self.peer, self.orch, "BLOCKER_RESOLVED")
            return {"blocker_wake": notice, "scripted_orchestrator_answer": outcome, "settled": settled, "resolved": resolved}
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
            self.native_answer(self.settle(message), "RESUMED_MASTER")
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
            wake = self.result_message(self.joint, self.claude, self.orch, "RESTART_WAKE")
            for message, reply in zip((self.master_one, self.master_two), self.master_replies):
                assert self.cli("message", "status", message)["requests"][0]["reply"] == reply
            return {"durable_final_before_restart": final, "job": job, "wake": wake}
        self.case("restart-before-worker-wake", restart_before_wake)

    def partial_case(self):
        def reject_partial():
            message = self.send(self.room, self.worker, "fake:partial")
            poll(lambda: self.cli("message", "status", message), lambda s: s["requests"][0]["start_bound"])
            poll(lambda: next(a for a in self.cli("diagnostics")["agents"] if a["agent_id"] == self.worker),
                 lambda a: a["status"] == "idle")
            deadline = time.monotonic() + 3
            reads = 0
            while time.monotonic() < deadline:
                status = self.cli("message", "status", message)
                assert not status["complete"] and status["requests"][0]["reply"] is None, status
                reads += 1
                time.sleep(0.1)
            return {"message": message, "expected": "pending", "stable_reads": reads, "status": status}
        self.case("partial-jsonl-never-publishes-success", reject_partial)

    def execute(self):
        try:
            self.start()
            self.worker_cases()
            self.joint_cases()
            self.resume_cases()
            self.partial_case()
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
