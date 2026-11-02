"""Opt-in native Claude/Codex/Kimi prompt acceptance; never builds Bus.

BUS_UNIVERSAL_DIALOG_LIVE=1 BUS_UNIVERSAL_DIALOG_BINARY=/absolute/copied/bus \
BUS_UNIVERSAL_DIALOG_SHA=<full-HEAD> \
    python3 tests/universal_dialog_live_acceptance_test.py -v

Real HOME supplies existing logins in place. Each run has a new private Bus and
three never-trusted projects. Only native trust answers may persist trust; this
runner never reads/copies credentials or writes provider configuration.
"""

import gzip
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import secrets
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
import unittest


REPO = Path(__file__).resolve().parents[1]
CASES = {"claude-trust", "codex-trust", "kimi-trust", "claude-permission",
         "codex-permission", "codex-question"}
PROVIDERS = {"claude": Path.home() / ".local/bin/claude",
             "codex": Path("/opt/homebrew/bin/codex"),
             "kimi": Path.home() / ".kimi-code/bin/kimi"}
FORBIDDEN = {"agent.dialog.observe", "agent.dialog.choose", "agent.dialog.answer",
             "agent.choose", "agent.answer", "agent.send_keys", "agent.send-keys",
             "pane.send_input", "pane.send-input", "pane.send_text", "pane.send-text",
             "pane.send_keys", "pane.send-keys"}
FIELDS = re.compile(r'([a-zA-Z_]+)=(?:"([^"\n]*)"|([^\s{}]+))')


def helper(filename):
    spec = importlib.util.spec_from_file_location(filename, Path(__file__).with_name(filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def menu_rows(observation):
    """Labels exactly as visible, with only menu markers/numbers removed."""
    rows = []
    for row, line in enumerate(observation["text"].splitlines()):
        text = line.strip().strip("│┃").strip()
        marked = text.startswith(("❯", "›", ">", "●", "◉", "▶", "→"))
        if marked:
            text = text[1:].lstrip()
        numbered = re.match(r"\d+[.):]\s+(.+)", text)
        label = numbered[1] if numbered else text
        if label:
            rows.append((row, label, marked or bool(numbered)))
    return rows


def route_proof(traces, actions):
    """Every action must match its own native driver result, including metadata."""
    events, alternate = {}, []
    for path in traces:
        raw = gzip.decompress(path.read_bytes()) if path.suffix == ".gz" else path.read_bytes()
        for line in raw.decode(errors="replace").splitlines():
            pairs = [(key, quoted or bare) for key, quoted, bare in FIELDS.findall(line)]
            fields = dict(pairs)
            if fields.get("event") == "bus.terminal.driver.result":
                request = fields.get("control_request_id")
                events.setdefault(request, []).append(fields)
            forbidden = {value for key, value in pairs if key == "method"} & FORBIDDEN
            if forbidden:
                alternate.append(sorted(forbidden))
    verified, missing = [], []
    seen = set()
    for action in actions:
        request = action.get("request_id")
        matches = events.get(request, [])
        valid = bool(request and request not in seen and action.get("ok")
                     and action.get("method") == "agent.press"
                     and action.get("outcome") in {"selected", "sent"}
                     and not action.get("selection_pending")
                     and (action.get("key_count", 0) or action.get("text_written")))
        valid = valid and len(matches) == 1 and all(
            matches[0].get(key) == str(action.get(key)).lower()
            if isinstance(action.get(key), bool)
            else matches[0].get(key) == str(action.get(key))
            for key in ["terminal_id", "pane_id", "outcome", "input_mode", "key_count", "text_written"]
        )
        seen.add(request)
        (verified if valid else missing).append(action)
    covered = {action["case"] for action in verified if action.get("acceptance")
               and (action["input_mode"] == "text" and action["text_written"]
                    if action["case"] == "codex-question" else action["input_mode"] == "select")}
    return {"passed": not missing and not alternate and covered == CASES,
            "covered": sorted(covered), "missing_cases": sorted(CASES - covered),
            "unverified_actions": missing, "alternate_input_methods": alternate,
            "verified_request_ids": [action["request_id"] for action in verified]}


class RouteProofTests(unittest.TestCase):
    def test_receipts_alone_or_mismatched_native_metadata_cannot_prove_routes(self):
        actions = [{"case": case, "acceptance": True, "method": "agent.press",
                    "request_id": f"request-{index}", "ok": True,
                    "terminal_id": f"terminal-{index}", "pane_id": f"pane-{index}",
                    "outcome": "selected", "input_mode": "select", "key_count": 1,
                    "text_written": False, "selection_pending": False}
                   for index, case in enumerate(sorted(CASES))]
        question = next(action for action in actions if action["case"] == "codex-question")
        question.update(outcome="sent", input_mode="text", text_written=True)
        self.assertFalse(route_proof([], actions)["passed"])
        with tempfile.TemporaryDirectory() as directory:
            trace = Path(directory) / "native.log"
            lines = []
            for action in actions:
                fields = {"event": "bus.terminal.driver.result",
                          "control_request_id": action["request_id"],
                          **{key: action[key] for key in ["terminal_id", "pane_id", "outcome",
                              "input_mode", "key_count", "text_written"]}}
                lines.append(" ".join(f"{key}={str(value).lower() if isinstance(value, bool) else value}"
                                      for key, value in fields.items()))
            good = "\n".join(lines) + "\n"
            trace.write_text(good)
            self.assertTrue(route_proof([trace], actions)["passed"])
            for replacement in [good.replace("key_count=1", "key_count=2", 1),
                                good.replace("terminal_id=terminal-0", "terminal_id=wrong", 1),
                                good + "method=agent.send_keys request_id=alternate\n",
                                "\n".join(lines[:-1])]:
                trace.write_text(replacement)
                self.assertFalse(route_proof([trace], actions)["passed"])
            trace.write_text(good)
            repeated = [dict(action) for action in actions]
            repeated[-1]["request_id"] = repeated[0]["request_id"]
            self.assertFalse(route_proof([trace], repeated)["passed"])


@unittest.skipUnless(os.environ.get("BUS_UNIVERSAL_DIALOG_LIVE") == "1",
                     "real providers require BUS_UNIVERSAL_DIALOG_LIVE=1")
class UniversalDialogLiveAcceptance(unittest.TestCase):
    def test_native_trust_permissions_and_typed_question_use_the_driver(self):
        sha = os.environ["BUS_UNIVERSAL_DIALOG_SHA"]
        self.assertRegex(sha, r"^[0-9a-f]{40}$")
        self.assertEqual(subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO,
                                                text=True).strip(), sha)
        binary = Path(os.environ["BUS_UNIVERSAL_DIALOG_BINARY"]).resolve(strict=True)
        self.assertFalse("target" in binary.parts or ".cargo-targets" in binary.parts,
                         "supply a copied, verified binary outside shared Cargo targets")
        evidence = REPO / "temp/universal-dialog/live" / sha / (
            time.strftime("%Y%m%dT%H%M%S") + "-" + secrets.token_hex(4))
        evidence.mkdir(mode=0o700, parents=True)
        root = Path(tempfile.mkdtemp(prefix="bus-ud-live-", dir="/private/tmp"))
        root.chmod(0o700)
        terminal, started, stopped, completed = None, False, False, set()
        actions, cleanup_errors = [], []
        support = helper("dialog_driver_acceptance_support_test.py")
        terminal_helper = helper("kimi_trust_acceptance_test.py")

        def save(name, value):
            path = evidence / (name + ".json")
            path.write_text(json.dumps(value, indent=2) + "\n")
            path.chmod(0o600)
            return value

        copied = root / "bin/bus"
        mapping = {**PROVIDERS, "node": Path("/opt/homebrew/bin/node")}
        env = {"HOME": str(Path.home()), "PATH": f"{root / 'bin'}:/usr/bin:/bin",
               "SHELL": "/bin/zsh", "ZDOTDIR": str(root / "zsh"),
               "TERM": "xterm-256color", "LANG": "en_US.UTF-8",
               "BUS_DATA_DIR": str(root / "data")}

        def run(args, optional=False):
            result = subprocess.run([str(copied), *args], cwd=root / "project", env=env,
                                    text=True, capture_output=True, timeout=30)
            if optional and result.returncode:
                return None
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            return result.stdout

        def control(*args, optional=False):
            raw = run(args, optional)
            if raw is None:
                return None
            value = json.loads(raw)
            if args[0] not in {"--paths", "stop"}:
                self.assertTrue(value.get("ok"), value)
            return value

        def wait(description, poll, timeout=120):
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                terminal.pump(0.2)
                value = poll()
                if value:
                    return value
            self.fail(f"Timed out: {description}; evidence={evidence}")

        def observe(agent, name):
            deadline = time.monotonic() + 3
            while True:
                result = subprocess.run([str(copied), "agent", "observe", agent],
                                        cwd=root / "project", env=env, text=True,
                                        capture_output=True, timeout=30)
                value = json.loads(result.stdout)
                if result.returncode == 0 and value.get("ok"):
                    return save(name + "-screen", value["result"])
                save(name + "-read-refusal", value)
                error = value.get("error", {})
                redrawing = (error.get("code") == "terminal_observation_unavailable"
                             or error.get("message") == "Terminal is redrawing; observe it again")
                self.assertTrue(redrawing and time.monotonic() < deadline,
                                f"unreadable native screen; evidence={evidence}")
                terminal.pump(0.2)

        def press(case, agent, before, *args, acceptance=False):
            name = f"{case}-{len(actions):02d}"
            save(name + "-before", before)
            receipt = save(name + "-press", control(
                "agent", "press", agent, "--terminal-id", before["terminal_id"], *args))
            result = receipt["result"]
            action = {"case": case, "acceptance": acceptance, "method": "agent.press",
                      "request_id": receipt["id"], "ok": receipt["ok"],
                      "terminal_id": result["observation"]["terminal_id"],
                      "pane_id": result["observation"]["pane_id"],
                      "outcome": result["outcome"], "key_count": len(result["keys"]),
                      "input_mode": "select" if args[0] == "--select" else (
                          "text" if "--text" in args else "keys"),
                      "text_written": result["text_written"],
                      "selection_pending": result["selection_pending"], "args": list(args)}
            actions.append(action)
            save("actions", actions)
            self.assertEqual(action["terminal_id"], before["terminal_id"])
            self.assertEqual(action["pane_id"], before["pane_id"])
            self.assertFalse(action["selection_pending"], f"unfinished input: {name}")
            self.assertEqual(action["outcome"], "selected" if args[0] == "--select" else "sent")
            return result

        def unique_label(screen, predicate):
            labels = [label for _, label, _ in menu_rows(screen) if predicate(label)]
            self.assertEqual(len(labels), 1, f"expected one actionable row; evidence={evidence}")
            return labels[0]

        def ready(agent):
            state = control("state")["result"]
            row = next(item for item in state["agents"] if str(item["id"]) == agent)
            return row.get("status") == "idle" and not row.get("current_request")

        def send(agent, text, name):
            return str(save(name, control("send", "--room", room, "--to", agent,
                                          "--text", text))["result"]["message_id"])

        def settle(message, name, token):
            def poll():
                status = save(name, control("message", "status", message)["result"])
                replies = "\n".join((item.get("reply") or {}).get("text", "")
                                    for item in status.get("requests", []))
                return status if status.get("complete") and token in replies else None
            return wait(name, poll, int(os.environ.get("BUS_UNIVERSAL_DIALOG_TIMEOUT", "180")))

        try:
            for name in ["data", "bin", "project", "zsh"]:
                (root / name).mkdir(mode=0o700)
            (root / "zsh/.zshrc").write_text("")
            shutil.copy2(binary, copied)
            self.assertEqual(hashlib.sha256(binary.read_bytes()).digest(),
                             hashlib.sha256(copied.read_bytes()).digest())
            # /opt/homebrew/bin also has lark-cli. Expose only approved executable
            # links, plus the interpreter required by Codex's /usr/bin/env node.
            for name, path in mapping.items():
                self.assertTrue(os.access(path, os.X_OK), f"approved executable missing: {path}")
                (root / "bin" / name).symlink_to(path)
            for directory in env["PATH"].split(":"):
                self.assertFalse(os.access(Path(directory) / "lark-cli", os.X_OK))

            version = run(["--version"])
            self.assertIn(sha, version)
            self.assertNotIn(sha + "-dirty", version, "acceptance requires the committed head")
            paths = save("paths", control("--paths"))
            self.assertEqual(Path(paths["data"]), root / "data")
            logs = Path(paths["logs"]).resolve()
            self.assertTrue(logs.is_relative_to(root / "data"))
            for provider in PROVIDERS:
                help_text = subprocess.check_output([str(root / "bin" / provider), "--help"],
                                                    env=env, text=True, stderr=subprocess.STDOUT)
                save(provider + "-cli", {"version": subprocess.check_output(
                    [str(root / "bin" / provider), "--version"], env=env, text=True).strip(),
                    "help": help_text})
            self.assertIn("manual", json.loads((evidence / "claude-cli.json").read_text())["help"])
            self.assertIn("on-request", json.loads((evidence / "codex-cli.json").read_text())["help"])
            bus_help = run(["agent", "add", "--help"])
            for provider in PROVIDERS:
                self.assertRegex(bus_help, rf"\b{provider}\b")
            self.assertIn("--select", run(["agent", "press", "--help"]))
            save("preflight", {"head": sha, "version": version.strip(), "scratch": str(root),
                 "home": env["HOME"], "path": env["PATH"], "lark_cli_resolves": False,
                 "executable_mapping": {key: str(value) for key, value in mapping.items()},
                 "binary_sha256": hashlib.sha256(copied.read_bytes()).hexdigest()})
            start_ms = int(time.time() * 1000)
            terminal = terminal_helper.Terminal([str(copied), "--dev"], root / "project", env)
            started = True
            state = wait("private Bus startup", lambda: control("state", optional=True), 45)["result"]
            self.assertEqual(Path(state["build"]["binary"]).resolve(), copied)
            self.assertFalse(state["launch"]["yolo"])
            identity = []
            for role in ["client", "server"]:
                starts = [json.loads(line) for file in logs.rglob(role + ".jsonl")
                          for line in file.read_text().splitlines()
                          if line.strip() and json.loads(line).get("kind") == "run_start"]
                self.assertEqual(len(starts), 1, f"unambiguous {role} identity required")
                record = starts[0]
                self.assertEqual(record["role"], role)
                self.assertEqual(record["build_sha"], sha)
                self.assertGreaterEqual(record["ts_ms"], start_ms)
                self.assertTrue(record["dev_tools"])
                os.kill(record["pid"], 0)
                if role == "client":
                    self.assertEqual(record["pid"], terminal.process.pid)
                identity.append(record)
            save("build-identity", identity)
            room = str(save("room", control("room", "create", "native-driver"))["result"]["room_id"])
            agents, projects = {}, {}
            for provider in PROVIDERS:
                project = root / "project" / provider
                project.mkdir(mode=0o700)
                subprocess.run(["/usr/bin/git", "init", "-q", str(project)], check=True, env=env)
                args = {"claude": "--permission-mode manual",
                        "codex": "--sandbox read-only --ask-for-approval on-request",
                        "kimi": ""}[provider]
                command = ["agent", "add", "--room", room, "--name", provider + "-native",
                           "--provider", provider, "--pwd", str(project), "--consent-hooks"]
                if args:
                    command += ["--args", args]
                agent = str(save(provider + "-add", control(*command))["result"]["agent_id"])
                agents[provider], projects[provider] = agent, project
                case = provider + "-trust"

                def trust_screen():
                    screen = observe(agent, case)
                    text = screen["text"].lower()
                    return screen if "trust" in text and str(project) in screen["text"] else None

                screen = wait(case, trust_screen, 90)
                tests = {"claude": lambda label: bool(re.fullmatch(
                             r"Yes, (?:I )?trust this folder[.!]?", label)),
                         "codex": lambda label: label == "Yes, continue",
                         "kimi": lambda label: label == "Trust and continue"}
                label = unique_label(screen, tests[provider])
                press(case, agent, screen, "--select", label, acceptance=True)
                wait(provider + " native composer after trust", lambda: ready(agent), 60)
                completed.add(case)

            for provider in ["claude", "codex"]:
                agent, project = agents[provider], projects[provider]
                case = provider + "-permission"
                token = "APPROVED_" + secrets.token_hex(6)
                marker = project / "approved.txt"
                command = f"printf '%s\\n' {shlex.quote(token)} > {shlex.quote(str(marker))}"
                escalation = ("Use exec_command with sandbox_permissions=require_escalated and a "
                              "justification so the native command approval is shown. "
                              if provider == "codex" else "Use the Bash tool. ")
                message = send(agent, "Native approval acceptance in this disposable project. "
                    "Do not use skills, delegate, read other files, change permissions, or bypass approval. "
                    + escalation + f"Run exactly this harmless command once: {command}. "
                    f"Wait for approval. After it succeeds, reply exactly {token}.", case + "-send")

                predicate = (lambda label: label == "Yes") if provider == "claude" else (
                    lambda label: label in {"Yes, proceed", "Yes, proceed (y)", "Approve once"})

                def permission_screen():
                    screen = observe(agent, case)
                    rows = [label for _, label, option in menu_rows(screen) if option]
                    return screen if (token in screen["text"] and screen.get("focus")
                                      and "esc" in screen["text"].lower()
                                      and any(predicate(label) for label in rows)) else None

                screen = wait(case, permission_screen)
                self.assertFalse(marker.exists(), "command ran before native approval")
                label = unique_label(screen, predicate)
                press(case, agent, screen, "--select", label, acceptance=True)
                wait(case + " command executes", lambda: marker.exists())
                self.assertEqual(marker.read_text(), token + "\n")
                settle(message, case + "-status", token)
                completed.add(case)

            agent, case = agents["codex"], "codex-question"
            before = observe(agent, case + "-plan-mode")
            press(case, agent, before, "--text", "/plan", "--submit")
            def plan_mode():
                screen = observe(agent, case + "-plan-mode")
                return screen if "plan mode" in screen["text"].lower() else None

            wait("Codex plan mode", plan_mode, 30)
            token = "TYPED_" + secrets.token_hex(6)
            title = "Which token should this scratch acceptance use?"
            message = send(agent, "Stay in Plan mode. Do not use skills, delegate, run commands or edit files. "
                f"Call native request_user_input with exactly one question titled {title!r}, "
                "offer Alpha and Beta and allow its normal custom Other answer. Wait for the human "
                "answer rather than answering yourself. After the answer, repeat its exact text in "
                "your final reply without further questions or implementation.", case + "-send")
            opened = False

            def question_screen():
                nonlocal opened
                screen = observe(agent, case)
                text = screen["text"]
                other = [label for _, label, option in menu_rows(screen) if option and label == "Other"]
                if title in text and other and re.search(r"(?:enter|↵).*submit", text, re.I):
                    return screen
                if not opened and title in text and re.search(r"(?:⇧←|shift\+left) to answer", text, re.I):
                    press(case, agent, screen, "--key", "shift+left")
                    opened = True
                return None

            screen = wait("native Codex question with custom row", question_screen)
            for step in range(8):
                focus = screen.get("focus")
                self.assertTrue(focus, "custom-answer navigation needs unique visible focus")
                if focus[1] == "Other":
                    break
                row = next(row for row, label, option in menu_rows(screen) if option and label == "Other")
                old_focus = focus
                press(case, agent, screen, "--key", "down" if row > focus[0] else "up")
                def changed_focus():
                    current = observe(agent, case)
                    return current if current.get("focus") != old_focus else None

                screen = wait("custom row navigation redraw", changed_focus, 10)
            self.assertEqual(screen["focus"][1], "Other", "custom field was not reached")
            self.assertNotIn(token, screen["text"], "answer must originate from this typed action")
            press(case, agent, screen, "--text", token, "--submit", acceptance=True)
            settle(message, case + "-status", token)
            completed.add(case)
        finally:
            primary = sys.exc_info()[1]
            traces = []
            if terminal is not None:
                try:
                    terminal.pump(0.2)
                    (evidence / "bus-terminal.ansi").write_bytes(terminal.output)
                except Exception as error:
                    cleanup_errors.append("terminal evidence: " + str(error))
            if started:
                try:
                    stop = save("stop", control("stop", optional=True) or {"stopped": False})
                    stopped = bool(stop.get("stopped"))
                except Exception as error:
                    cleanup_errors.append("private stop: " + str(error))
            else:
                stopped = True
            if terminal is not None:
                try:
                    terminal.close()
                except Exception as error:
                    cleanup_errors.append("client close: " + str(error))
            try:
                traces = support.collect_logs(root / "data", evidence)
            except Exception as error:
                cleanup_errors.append("log evidence: " + str(error))
            try:
                proof = route_proof(traces, actions)
            except Exception as error:
                cleanup_errors.append("route evidence: " + str(error))
                proof = {"passed": False, "error": str(error)}
            save("driver-route-proof", proof)
            clean = stopped and not cleanup_errors
            if clean:
                try:
                    shutil.rmtree(root)
                except OSError as error:
                    cleanup_errors.append("scratch removal: " + str(error))
                    clean = False
            if not clean:
                save("retained-scratch", {"path": str(root), "errors": cleanup_errors})
            passed = primary is None and clean and completed == CASES and proof["passed"]
            save("summary", {"head": sha, "result": "PASS" if passed else "FAIL",
                 "cases_completed": sorted(completed), "driver_routes": proof,
                 "cleanup": {"stopped": stopped, "scratch_removed": not root.exists(),
                             "errors": cleanup_errors}, "error": str(primary) if primary else None})
            print(f"acceptance_evidence={evidence}", flush=True)
        self.assertTrue(passed, f"native acceptance or route proof failed; evidence={evidence}")


if __name__ == "__main__":
    unittest.main()
