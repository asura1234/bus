#!/usr/bin/env python3
"""End-to-end Bus message round trips with real Claude Code, Codex and Cursor.

Starts a throwaway Bus (fresh BUS_DATA_DIR under temp/e2e/) in a pseudo-terminal,
drives it only through the dev CLI, and shuts it down at the end. Agents work in
a scratch git repository under temp/e2e/workspace. Spends real model usage, so
--allow-live-models is required. Writes temp/e2e/<timestamp>/report.json.
"""
import argparse
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import shutil
import signal
import struct
import subprocess
import sys
import termios
import threading
import time
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, str(Path(__file__).resolve().parent))
from bus_dev_acceptance import round_trip_prompt  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
E2E = ROOT / "temp/e2e"
WORKSPACE = E2E / "workspace"
# Provider name -> the executable Bus launches for it.
PROVIDERS = {"claude": "claude", "codex": "codex", "cursor": "cursor-agent"}
CASES = ("single", "queued", "multi", "background", "dialog", "resume")
SCRUB_PREFIXES = ("BUS_", "HERDR_", "CLAUDE_CODE_")
SCRUB_NAMES = ("CLAUDECODE",)


class CliError(AssertionError):
    pass


class Skip(Exception):
    pass


def installed_providers(which=shutil.which):
    return [name for name, command in PROVIDERS.items() if which(command)]


def split_list(value, allowed, what):
    items = [item.strip() for item in value.split(",") if item.strip()]
    unknown = [item for item in items if item not in allowed]
    if unknown or not items:
        raise ValueError(f"unknown {what}: {', '.join(unknown) or value!r}; choose from {', '.join(allowed)}")
    return [item for item in allowed if item in items]


def parse_args(argv=None, which=shutil.which):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--allow-live-models", action="store_true", required=True,
                        help="Required: the run spends real model usage")
    parser.add_argument("--providers", help="Comma list of claude,codex,cursor (default: all installed)")
    parser.add_argument("--cases", default=",".join(CASES), help="Comma list of " + ",".join(CASES))
    parser.add_argument("--keep", action="store_true", help="Leave the Bus data directory for debugging")
    parser.add_argument("--binary", help="Bus binary (default: target/debug/bus, else target/debug/herdr)")
    parser.add_argument("--timeout", type=float, default=300, help="Seconds per message to settle (default: 300)")
    args = parser.parse_args(argv)
    try:
        args.providers = split_list(args.providers or ",".join(installed_providers(which)), tuple(PROVIDERS),
                                    "provider")
        args.cases = split_list(args.cases, CASES, "case")
    except ValueError as error:
        parser.error(str(error))
    missing = [p for p in args.providers if not which(PROVIDERS[p])]
    if missing:
        parser.error("provider CLI not installed: " + ", ".join(missing))
    if not 10 <= args.timeout <= 3600:
        parser.error("--timeout must be between 10 and 3600 seconds")
    return args


def find_binary(explicit=None, root=ROOT):
    candidates = [Path(explicit)] if explicit else [root / "target/debug/bus", root / "target/debug/herdr"]
    for path in candidates:
        if path.is_file() and os.access(path, os.X_OK):
            return path.resolve()
    raise SystemExit("No Bus binary: build with cargo build, or pass --binary PATH (tried "
                     + ", ".join(map(str, candidates)) + ")")


def bus_argv(binary):
    # The herdr-named binary needs --bus; the renamed bus binary launches Bus directly.
    return [str(binary), "--bus", "--dev"] if Path(binary).name == "herdr" else [str(binary), "--dev"]


def scrubbed_env(environ, data_dir):
    env = {k: v for k, v in environ.items() if not k.startswith(SCRUB_PREFIXES) and k not in SCRUB_NAMES}
    env.update(BUS_DATA_DIR=str(data_dir), TERM="xterm-256color")
    return env


def case_support(provider, case, providers):
    """Return a SKIP reason, or None when the provider can run the case."""
    if case == "background" and provider != "claude":
        return "background shells that outlive a turn are a Claude Code feature"
    if case == "multi" and len(providers) < 2:
        return "needs at least two providers"
    return None


def pick_option(dialog, purpose):
    """Choose a dialog option, or raise for a dialog this test does not expect.

    Update prompts are skipped (never install anything); folder trust is
    accepted; permission prompts are allowed once, never "always".
    """
    options = dialog["options"]
    labels = {option["number"]: option["label"].strip() for option in options}
    if re.search(r"update available|new version", dialog.get("text") or "", re.I):
        wanted = [n for n, label in labels.items() if re.fullmatch(r"skip", label, re.I)]
    else:
        accept = r"(yes|trust|continue)\b" if purpose == "trust" else r"(yes|allow|run|proceed|approve)\b"
        deny = re.compile(r"\bno\b|always|don.t ask|session|every|skip|exit|update|install", re.I)
        wanted = [n for n, label in labels.items() if re.match(accept, label, re.I) and not deny.search(label)]
    if not wanted:
        raise AssertionError(f"unexpected {purpose} dialog: {dialog}")
    return wanted[0]


def dialog_prompt(provider, token):
    # Bus rejects permission-mode launch args, and Claude's auto mode approves
    # most commands on its own, so Claude raises a question dialog instead.
    if provider == "claude":
        return ("We are testing Bus delivery of dialogs. Do not use skills. Use your AskUserQuestion tool "
                "once to ask \"Continue the Bus e2e check?\" with exactly two options, Yes and No. "
                f"After the answer, reply with exactly {token} and nothing else.")
    if provider == "codex":
        command = f"ln -s README.md ../e2e-link-{token}"
        how = ("It writes outside the workspace, so run it with escalated sandbox permissions "
               "(request approval); do not pick another path.")
    else:
        command = f"ln -s README.md e2e-link-{token}"
        how = "Use your shell tool; wait for approval if asked."
    return ("We are testing Bus delivery of permission prompts. Do not use skills. "
            f"Run exactly this shell command once: `{command}`. {how} "
            f"Then reply with exactly {token} and nothing else.")


def background_prompt(token):
    return ("We are testing Bus delivery. Do not use skills. Start the shell command `sleep 20` "
            "with your Bash tool's run_in_background option, do not wait for it, "
            f"and reply with exactly {token} and nothing else.")


def format_table(rows):
    header = ("provider", "case", "result", "seconds", "detail")
    lines = [header] + [(r["provider"], r["case"], r["result"],
                         "" if r.get("seconds") is None else f"{r['seconds']:.1f}",
                         (r.get("reason") or "")[:90]) for r in rows]
    widths = [max(len(line[i]) for line in lines) for i in range(4)]
    return "\n".join("  ".join(cell.ljust(widths[i]) if i < 4 else cell for i, cell in enumerate(line)).rstrip()
                     for line in lines)


def new_row(provider, case):
    return {"provider": provider, "case": case, "result": "FAIL", "seconds": None, "reason": None,
            "commands": [], "last_status": None}


def summarize(rows):
    counts = {result: sum(r["result"] == result for r in rows) for result in ("PASS", "FAIL", "SKIP")}
    return {"passed": counts["FAIL"] == 0 and counts["PASS"] > 0, **counts}


def pids_holding(paths):
    pids = set()
    for path in paths:
        if Path(path).exists():
            out = subprocess.run(["lsof", "-t", "+D", str(path)], capture_output=True, text=True).stdout
            pids |= {int(pid) for pid in out.split()}
    return pids - {os.getpid()}


class Bus:
    """One throwaway Bus: a TUI client in a pty plus the dev CLI against its data dir."""

    def __init__(self, binary, run_dir, env):
        self.binary, self.run_dir, self.env = binary, run_dir, env
        self.data = Path(env["BUS_DATA_DIR"])
        self.child = None
        self.master = None
        self.pump_thread = None

    def start(self):
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 45, 160, 0, 0))
        tty_log = open(self.run_dir / "tui.log", "ab")
        self.child = subprocess.Popen(bus_argv(self.binary), stdin=slave, stdout=slave, stderr=slave,
                                      env=self.env, cwd=str(WORKSPACE), start_new_session=True)
        os.close(slave)
        master = self.master

        def pump():  # Keep draining the TUI so it never blocks on output.
            while True:
                try:
                    if select.select([master], [], [], 0.2)[0]:
                        chunk = os.read(master, 65536)
                        if not chunk:
                            break
                        tty_log.write(chunk[-4096:] if len(chunk) > 4096 else chunk)
                except OSError:
                    break
            tty_log.close()

        self.pump_thread = threading.Thread(target=pump, daemon=True)
        self.pump_thread.start()
        deadline = time.monotonic() + 30
        while True:
            try:
                return self.cli("state")
            except CliError:
                if self.child.poll() is not None or time.monotonic() > deadline:
                    raise
                time.sleep(0.5)

    def cli(self, *argv, timeout=60, row=None):
        command = [*map(str, argv)]
        started = time.monotonic()
        result = subprocess.run(bus_argv(self.binary) + command, capture_output=True, text=True,
                                env=self.env, timeout=timeout, check=False)
        try:
            response = json.loads(result.stdout)
        except ValueError:
            response = {"ok": False, "error": {"stdout": result.stdout[-2000:], "stderr": result.stderr[-2000:]}}
        entry = {"argv": command, "ok": bool(response.get("ok")),
                 "ms": round((time.monotonic() - started) * 1000)}
        if not entry["ok"]:
            entry["error"] = response.get("error")
        if row is not None:
            row["commands"].append(entry)
        if result.returncode or not response.get("ok"):
            raise CliError(response.get("error"))
        return response["result"]

    def stop(self):
        """Quit the TUI, stop the server, and kill anything still holding our directories."""
        report = {}
        try:
            self.cli("quit", timeout=15)
        except Exception as error:  # A dead Bus cannot quit; cleanup below still runs.
            report["quit_error"] = str(error)
        if self.child is not None:
            try:
                self.child.wait(timeout=15)
            except subprocess.TimeoutExpired:
                os.killpg(self.child.pid, signal.SIGTERM)
                self.child.wait(timeout=5)
        # The headless server outlives the client; stop it so agents exit too.
        subprocess.run([str(self.binary), "session", "stop", "bus"], env=self.env, capture_output=True,
                       timeout=20, check=False)
        killed = []
        for sig in (signal.SIGTERM, signal.SIGKILL):
            deadline = time.monotonic() + 5
            while pids := pids_holding([self.data, WORKSPACE]):
                if time.monotonic() > deadline:
                    for pid in pids:
                        try:
                            os.kill(pid, sig)
                            killed.append(pid)
                        except ProcessLookupError:
                            pass
                    time.sleep(1)
                    break
                time.sleep(0.5)
        if self.master is not None:
            try:
                os.close(self.master)
            except OSError:
                pass
            self.master = None
        report["killed"] = sorted(set(killed))
        report["leftover"] = sorted(pids_holding([self.data, WORKSPACE]))
        return report


class Run:
    def __init__(self, args):
        self.args = args
        self.binary = find_binary(args.binary)
        self.run_dir = E2E / time.strftime("%Y%m%d-%H%M%S")
        self.run_dir.mkdir(parents=True, mode=0o700)
        self.data = self.run_dir / "data"
        self.data.mkdir(mode=0o700)
        self.env = scrubbed_env(os.environ, self.data)
        self.lock = threading.Lock()
        self.rows = []
        self.report = {"started_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"), "binary": str(self.binary),
                       "data_dir": str(self.data), "workspace": str(WORKSPACE),
                       "providers": args.providers, "cases": args.cases, "versions": {},
                       "setup": {}, "dialogs": [], "rows": self.rows}
        self.bus = Bus(self.binary, self.run_dir, self.env)
        self.agents = {}  # provider -> agent id
        self.sessions = {}  # provider -> last provider session id
        self.room = None

    def save(self):
        with self.lock:
            (self.run_dir / "report.json").write_text(json.dumps(self.report, indent=2) + "\n")

    # Setup ---------------------------------------------------------------

    def make_workspace(self):
        if WORKSPACE.exists():
            shutil.rmtree(WORKSPACE)
        for link in E2E.glob("e2e-link-*"):  # Codex's dialog case writes outside the workspace.
            link.unlink()
        WORKSPACE.mkdir(parents=True)
        (WORKSPACE / "README.md").write_text("Scratch repository for Bus end-to-end tests.\n")
        git = ["git", "-C", str(WORKSPACE), "-c", "user.name=bus-e2e", "-c", "user.email=bus-e2e@invalid"]
        subprocess.run(["git", "init", "-q", str(WORKSPACE)], check=True)
        subprocess.run(git + ["add", "README.md"], check=True)
        subprocess.run(git + ["commit", "-q", "-m", "init"], check=True)

    def answer_dialogs(self, provider, purpose, row=None):
        agent = self.agents[provider]
        found = self.bus.cli("agent", "dialog", agent, row=row)
        if not found.get("dialog"):
            return False
        option = pick_option(found["dialog"], purpose)
        result = self.bus.cli("agent", "choose", agent, "--option", option, "--fingerprint",
                              found["fingerprint"], row=row)
        with self.lock:
            self.report["dialogs"].append({"provider": provider, "purpose": purpose, "dialog": found["dialog"],
                                           "option": option, "outcome": result.get("outcome")})
        return True

    def screen(self, provider):
        try:
            return self.bus.cli("agent", "read", self.agents[provider], "--source", "visible")["text"]
        except Exception as error:
            return f"unreadable: {error}"

    def wait_ready(self, providers, record, timeout=240):
        started = time.monotonic()
        pending = set(providers)
        while pending:
            agents = {a["agent_id"]: a for a in self.bus.cli("diagnostics")["agents"]}
            for provider in sorted(pending):
                agent = agents.get(self.agents[provider])
                if agent and agent["reason"] is None and agent["status"] == "idle":
                    pending.discard(provider)
                    record[provider] = {"ready_after_s": round(time.monotonic() - started, 1),
                                        "identity": agent["identity"]}
                else:
                    self.answer_dialogs(provider, "trust")
                    record[provider] = {"waiting": agent}
            if pending and time.monotonic() - started > timeout:
                for provider in pending:
                    record[provider]["screen"] = self.screen(provider)
                raise AssertionError("agents not ready: " + ", ".join(sorted(pending)))
            time.sleep(1)
        self.save()

    def setup(self):
        for provider in self.args.providers:
            out = subprocess.run([PROVIDERS[provider], "--version"], capture_output=True, text=True,
                                 timeout=20, check=False)
            self.report["versions"][provider] = (out.stdout or out.stderr).strip()[:200]
        self.make_workspace()
        self.bus.start()
        self.room = self.bus.cli("room", "create", "e2e")["room_id"]
        for provider in self.args.providers:
            self.agents[provider] = self.bus.cli(
                "agent", "add", "--room", self.room, "--name", provider, "--provider", provider,
                "--pwd", WORKSPACE, "--consent-hooks")["agent_id"]
        self.wait_ready(self.args.providers, self.report["setup"])

    # Messages --------------------------------------------------------------

    def token(self, provider, case, n=1):
        return f"E2E_{provider.upper()}_{case.upper()}_{n}_{os.urandom(3).hex().upper()}"

    def send(self, providers, text, row):
        to = ",".join(str(self.agents[p]) for p in providers)
        return self.bus.cli("send", "--room", self.room, "--to", to, "--text", text, row=row)

    def settle(self, message_ids, row):
        """Poll until every message completes, answering dialogs on the way."""
        deadline = time.monotonic() + self.args.timeout
        statuses = {}
        while True:
            for message_id in message_ids:
                if statuses.get(message_id, {}).get("complete"):
                    continue
                status = self.bus.cli("message", "status", message_id)
                statuses[message_id] = row["last_status"] = status
                for agent_id in status.get("waiting_on_dialog") or []:
                    provider = next(p for p, a in self.agents.items() if a == agent_id)
                    if self.answer_dialogs(provider, "allow", row):
                        row["dialogs_answered"] = row.get("dialogs_answered", 0) + 1
            if all(statuses[m]["complete"] for m in message_ids):
                return statuses
            if time.monotonic() > deadline:
                raise AssertionError(f"not settled within {self.args.timeout:.0f}s")
            time.sleep(0.5)

    def verify(self, status, tokens_by_agent, row):
        assert status["complete"], "message did not complete"
        requests = status["requests"]
        assert {r["agent_id"] for r in requests} == set(tokens_by_agent), "wrong recipients"
        for request in requests:
            name = request.get("agent_name")
            assert request["stage"] == "replied", f"{name}: stage {request['stage']}"
            assert not request.get("uncertain_outcome"), f"{name}: uncertain outcome"
            reply = (request.get("reply") or {}).get("text", "").strip()
            assert reply == tokens_by_agent[request["agent_id"]], f"{name}: reply {reply[:80]!r}"
            provider = next(p for p, a in self.agents.items() if a == request["agent_id"])
            if request.get("session_id"):
                self.sessions[provider] = request["session_id"]
        errors = {a["name"]: a["detail"] for a in self.bus.cli("diagnostics", row=row)["agents"]
                  if a["agent_id"] in tokens_by_agent and a["detail"]}
        assert not errors, f"agent error: {errors}"

    def round_trip(self, provider, row, case, text=None, n=1):
        token = self.token(provider, case, n)
        receipt = self.send([provider], text(token) if text else round_trip_prompt(token), row)
        statuses = self.settle([receipt["message_id"]], row)
        self.verify(statuses[receipt["message_id"]], {self.agents[provider]: token}, row)

    # Cases -------------------------------------------------------------------

    def case_single(self, provider, row):
        self.round_trip(provider, row, "single")

    def case_queued(self, provider, row):
        tokens, ids = [], []
        for n in range(1, 4):
            tokens.append(self.token(provider, "queued", n))
            ids.append(self.send([provider], round_trip_prompt(tokens[-1]), row)["message_id"])
        statuses = self.settle(ids, row)
        received = []
        for message_id, token in zip(ids, tokens):
            status = statuses[message_id]
            self.verify(status, {self.agents[provider]: token}, row)
            received.append(status["requests"][0]["reply"]["received_at_ms"])
        assert received == sorted(received), f"replies out of order: {received}"

    def case_background(self, provider, row):
        self.round_trip(provider, row, "background", text=background_prompt)
        # Let the background shell finish; Claude may start a turn of its own when it does.
        time.sleep(25)
        deadline = time.monotonic() + self.args.timeout
        while True:
            agent = next(a for a in self.bus.cli("diagnostics", row=row)["agents"]
                         if a["agent_id"] == self.agents[provider])
            if agent["status"] == "idle" and agent["reason"] is None:
                break
            if agent["status"] != "working":
                self.answer_dialogs(provider, "allow", row)
            assert time.monotonic() < deadline, f"agent stuck after background shell: {agent}"
            time.sleep(1)
        self.round_trip(provider, row, "background", n=2)

    def case_dialog(self, provider, row):
        self.round_trip(provider, row, "dialog", text=lambda token: dialog_prompt(provider, token))
        if not row.get("dialogs_answered"):
            raise Skip("the agent replied without raising a permission dialog; its permission "
                       "settings approved the command or it did not run it")

    def run_provider_cases(self, provider):
        for case in ("single", "queued", "background", "dialog"):
            if case not in self.args.cases:
                continue
            self.run_case(provider, case, getattr(self, "case_" + case))

    def run_case(self, provider, case, body):
        row = new_row(provider, case)
        with self.lock:
            self.rows.append(row)
        started = time.monotonic()
        try:
            reason = case_support(provider, case, self.args.providers)
            if reason:
                raise Skip(reason)
            body(provider, row)
            row["result"] = "PASS"
        except Skip as skip:
            row["result"], row["reason"] = "SKIP", str(skip)
        except Exception as error:  # Record and continue with the next case.
            row["reason"] = f"{type(error).__name__}: {error}"
            row["screen"] = self.screen(provider)
        row["seconds"] = round(time.monotonic() - started, 1)
        if row["result"] == "PASS":
            row["last_status"] = None
        print(f"{provider:7} {case:10} {row['result']:4} {row['seconds']:6.1f}s {row['reason'] or ''}",
              flush=True)
        self.save()

    def case_multi(self):
        providers = self.args.providers
        rows = {p: new_row(p, "multi") for p in providers}
        with self.lock:
            self.rows.extend(rows.values())
        started = time.monotonic()
        try:
            if len(providers) < 2:
                raise Skip(case_support(providers[0], "multi", providers))
            token = self.token("all", "multi")
            row = rows[providers[0]]
            receipt = self.send(providers, round_trip_prompt(token), row)
            statuses = self.settle([receipt["message_id"]], row)
            status = statuses[receipt["message_id"]]
            for provider in providers:
                one = {**status, "requests": [r for r in status["requests"]
                                              if r["agent_id"] == self.agents[provider]]}
                try:
                    self.verify(one, {self.agents[provider]: token}, rows[provider])
                    rows[provider]["result"] = "PASS"
                except AssertionError as error:
                    rows[provider]["reason"], rows[provider]["last_status"] = str(error), one
        except Skip as skip:
            for r in rows.values():
                r["result"], r["reason"] = "SKIP", str(skip)
        except Exception as error:
            for r in rows.values():
                r["reason"] = f"{type(error).__name__}: {error}"
                r["last_status"] = rows[providers[0]]["last_status"]
        for p, r in rows.items():
            r["seconds"] = round(time.monotonic() - started, 1)
            print(f"{p:7} {'multi':10} {r['result']:4} {r['seconds']:6.1f}s {r['reason'] or ''}", flush=True)
        self.save()

    def case_resume(self):
        before = {}
        for provider in self.args.providers:
            if provider not in self.sessions:
                row = new_row(provider, "resume")
                try:
                    self.round_trip(provider, row, "resume", n=0)
                except Exception:
                    pass
            before[provider] = self.sessions.get(provider)
        def terminal(record, provider):
            return ((record.get(provider) or {}).get("identity") or {}).get("terminal_id")
        terminals = {p: terminal(self.report["setup"], p) for p in self.args.providers}
        resume = self.report["resume"] = {"sessions_before": before, "stop": self.bus.stop(), "ready": {}}
        self.bus.start()
        try:
            self.wait_ready(self.args.providers, resume["ready"])
        except AssertionError as error:
            resume["ready_error"] = str(error)
        for provider in self.args.providers:
            def body(provider, row, old=before[provider]):
                if not old:
                    raise AssertionError("no session before the restart")
                self.round_trip(provider, row, "resume")
                new = self.sessions.get(provider)
                if new != old:
                    raise AssertionError(f"new session {new} instead of resuming {old}")
                if terminal(resume["ready"], provider) == terminals[provider]:
                    raise AssertionError("the agent kept its terminal; Bus did not restart it")
            self.run_case(provider, "resume", body)

    def execute(self):
        try:
            self.setup()
        except Exception as error:
            self.report["setup_error"] = f"{type(error).__name__}: {error}"
            for provider in self.args.providers:
                for case in self.args.cases:
                    row = new_row(provider, case)
                    row["reason"] = "setup failed: " + self.report["setup_error"]
                    self.rows.append(row)
            return
        with ThreadPoolExecutor(len(self.args.providers)) as pool:
            list(pool.map(self.run_provider_cases, self.args.providers))
        if "multi" in self.args.cases:
            self.case_multi()
        if "resume" in self.args.cases:
            self.case_resume()


def main(argv=None):
    args = parse_args(argv)
    E2E.mkdir(parents=True, exist_ok=True)
    lock = open(E2E / ".lock", "w")
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        raise SystemExit("another e2e run holds temp/e2e/.lock")
    run = Run(args)
    print(f"Bus e2e: {run.binary}  data {run.data}", flush=True)
    try:
        run.execute()
    finally:
        run.report["cleanup"] = run.bus.stop()
        if not args.keep:
            shutil.rmtree(run.data, ignore_errors=True)
            run.report["data_dir_removed"] = True
        run.report["summary"] = summarize(run.rows)
        run.save()
    order = {p: i for i, p in enumerate(PROVIDERS)}
    rows = sorted(run.rows, key=lambda r: (order[r["provider"]], CASES.index(r["case"])))
    print("\n" + format_table(rows))
    summary = run.report["summary"]
    print(f"\n{summary['PASS']} passed, {summary['FAIL']} failed, {summary['SKIP']} skipped; "
          f"report {run.run_dir / 'report.json'}")
    if run.report["cleanup"].get("leftover"):
        print("WARNING: processes still hold the run directories: " + str(run.report["cleanup"]["leftover"]))
    return 0 if summary["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
