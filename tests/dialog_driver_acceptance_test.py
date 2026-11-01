"""Opt-in real Kimi driving fake dialogs and an actual scratch zsh pane.

BUS_DIALOG_DRIVER_BINARY=/absolute/copied/bus BUS_DIALOG_DRIVER_SHA=<full-head> \
    python3 tests/dialog_driver_acceptance_test.py -v

Reads the approved real Kimi login in place. Never copies provider credentials.
No fixture answer is supplied by this runner: only real-driver startup trust.
"""

import contextlib
import hashlib
import io
import importlib.util
import json
import os
from pathlib import Path
import secrets
import shlex
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch


REPO = Path(__file__).resolve().parents[1]
FIXTURES = REPO / "tests/fixtures/dialog-driver"
helper = importlib.util.spec_from_file_location(
    "dialog_driver_terminal", REPO / "tests/kimi_trust_acceptance_test.py"
)
terminal_module = importlib.util.module_from_spec(helper)
helper.loader.exec_module(terminal_module)


SUPPORT_PATH = Path(__file__).with_name("dialog_driver_acceptance_support.py")
support_spec = importlib.util.spec_from_file_location("dialog_driver_acceptance_support", SUPPORT_PATH)
support = importlib.util.module_from_spec(support_spec)
support_spec.loader.exec_module(support)


def reply_text(status):
    return "\n".join(
        (request.get("reply") or {}).get("text", "")
        for request in status.get("requests", [])
    )


audit_records = support.audit_records

class AuditReaderTests(unittest.TestCase):
    def test_waits_for_the_complete_audit_record(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "fake-dialog-results.jsonl"
            prefix = b'{"id":"one","passed":true}\n'
            path.write_bytes(prefix + b'{"id":"two","answer":"\xe7')
            self.assertEqual(audit_records(path.parent), [{"id": "one", "passed": True}])
            path.write_bytes(prefix + b'{"id":"two","passed":true}\n')
            self.assertEqual([row["id"] for row in audit_records(path.parent)], ["one", "two"])


class _NoProcessTerminal:
    output = b""

    def __init__(self, *args, **kwargs):
        pass

    def pump(self, *args):
        pass

    def close(self):
        pass


class AcceptanceEvidenceTests(unittest.TestCase):
    def test_failed_rerun_does_not_report_a_previous_pass_as_its_summary(self):
        module = sys.modules[__name__]
        head = 'a' * 40
        with tempfile.TemporaryDirectory(prefix='bus-acceptance-evidence-probe-') as directory:
            root = Path(directory).resolve()
            repo = root / 'repo'
            evidence = repo / 'temp/tui-driver/acceptance' / head
            evidence.mkdir(parents=True)
            previous = {'head': head, 'result': 'PASS', 'dialogs': 15,
                        'real_provider': 'kimi', 'shell_answer': 'n'}
            (evidence / 'summary.json').write_text(json.dumps(previous))
            binary = root / 'copied-bus'
            binary.write_bytes(b'probe binary; never executed')
            private = root / 'private-runtime'
            captured = io.StringIO()
            calls = []
            def make_private(*args, **kwargs):
                private.mkdir(mode=0o700)
                return str(private)
            def output(command, **kwargs):
                calls.append(command)
                if command[0] == 'git':
                    return head + '\n' if command[1:3] == ['rev-parse', 'HEAD'] else ''
                if command == [str(binary), '--version']:
                    return 'bus v0.9.0 ' + head
                raise AssertionError('Unexpected external call: ' + repr(command))
            def reject_startup(command, **kwargs):
                calls.append(command)
                self.assertEqual(command[-1], '--paths')
                return subprocess.CompletedProcess(command, 1, '', 'forced private-startup failure')
            case = module.DialogDriverAcceptance('test_real_agent_clears_all_fake_dialogs_and_shell_prompt')
            try:
                with patch.object(module, 'REPO', repo), \
                     patch.object(module.Path, 'home', return_value=root / 'private-home'), \
                     patch.object(module.tempfile, 'mkdtemp', side_effect=make_private), \
                     patch.object(module.os, 'access', side_effect=lambda p, m: str(p).endswith('/.kimi-code/bin/kimi')), \
                     patch.object(module.subprocess, 'check_output', side_effect=output), \
                     patch.object(module.subprocess, 'run', side_effect=reject_startup), \
                     patch.object(module.terminal_module, 'Terminal', side_effect=AssertionError('No terminal may launch')), \
                     patch.dict(os.environ, {'BUS_DIALOG_DRIVER_BINARY': str(binary), 'BUS_DIALOG_DRIVER_SHA': head}, clear=True), \
                     contextlib.redirect_stdout(captured):
                    with self.assertRaisesRegex(AssertionError, 'forced private-startup failure'):
                        case.test_real_agent_clears_all_fake_dialogs_and_shell_prompt()
            finally:
                case.doCleanups()
            self.assertFalse(private.exists(), 'the mocked private runtime was cleaned')
            reported = [line.split('=', 1)[1] for line in captured.getvalue().splitlines()
                        if line.startswith('acceptance_evidence=')]
            self.assertEqual(len(reported), 1)
            summary = Path(reported[0]) / 'summary.json'
            observed = json.loads(summary.read_text()) if summary.exists() else None
            self.assertFalse(observed is not None and observed.get('result') == 'PASS',
                             'a failed attempt must not expose an earlier PASS as its own summary')

            self.assertEqual(json.loads((evidence / "summary.json").read_text()), previous)

    def test_failed_stop_keeps_primary_failure_and_reports_evidence(self):
        module = sys.modules[__name__]
        head = "b" * 40
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            repo = root / "repo"; repo.mkdir()
            binary = root / "copied-bus"; binary.write_bytes(b"never executed")
            private = root / "private"
            def make_private(*a, **k):
                private.mkdir(mode=0o700); return str(private)
            def output(command, **k):
                if command[0] == "git":
                    return head + "\n" if command[1:3] == ["rev-parse", "HEAD"] else ""
                return "bus v0 " + head
            def run(command, **k):
                if command[-1] == "--paths":
                    return subprocess.CompletedProcess(command, 0, '{"data": "%s"}' % (private / "data"), "")
                if command[-1] == "state":
                    raise RuntimeError("PRIMARY startup failure")
                return subprocess.CompletedProcess(command, 1, "", "stop failed")
            captured = io.StringIO()
            case = module.DialogDriverAcceptance("test_real_agent_clears_all_fake_dialogs_and_shell_prompt")
            raised = None
            try:
                with patch.object(module, "REPO", repo), \
                     patch.object(module.Path, "home", return_value=root / "home"), \
                     patch.object(module.tempfile, "mkdtemp", side_effect=make_private), \
                     patch.object(module.os, "access", side_effect=lambda p, m: str(p).endswith("/.kimi-code/bin/kimi")), \
                     patch.object(module.subprocess, "check_output", side_effect=output), \
                     patch.object(module.subprocess, "run", side_effect=run), \
                     patch.object(module.terminal_module, "Terminal", _NoProcessTerminal), \
                     patch.dict(os.environ, {"BUS_DIALOG_DRIVER_BINARY": str(binary), "BUS_DIALOG_DRIVER_SHA": head}, clear=True), \
                     contextlib.redirect_stdout(captured):
                    try:
                        case.test_real_agent_clears_all_fake_dialogs_and_shell_prompt()
                    except BaseException as error:
                        raised = error
            finally:
                case.doCleanups()
            self.assertIn("PRIMARY startup failure", str(raised), f"primary failure was masked by: {raised!r}")
            self.assertIn("acceptance_evidence=", captured.getvalue(), "evidence path was not reported")

            reported = captured.getvalue().split("acceptance_evidence=", 1)[1].splitlines()[0]
            retained = json.loads((Path(reported) / "retained-scratch.json").read_text())
            self.assertEqual(Path(retained["path"]), private)
            self.assertTrue(private.exists(), "an unconfirmed stop must retain its root")
            self.assertIn("stop failed", json.loads((Path(reported) / "stop-failure.json").read_text())["stderr"])


    def test_outcomes_without_driver_routes_cannot_pass(self):
        module = sys.modules[__name__]
        head = "c" * 40
        ids = [action["id"] for action in json.loads((FIXTURES / "suite.json").read_text())["turns"][0]["actions"] if action["type"] == "screen"]
        self.assertEqual(len(ids), 15, "update the closed mock alongside the prescribed suite")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            repo = root / "repo"; repo.mkdir()
            binary = root / "copied-bus"; binary.write_bytes(b"never executed")
            private = root / "private"
            completed = [False]
            def make_private(*args, **kwargs):
                private.mkdir(mode=0o700); return str(private)
            def output(command, **kwargs):
                if command[0] == "git":
                    return head + "\n" if command[1:3] == ["rev-parse", "HEAD"] else ""
                return "bus v0 " + head
            def run(command, **kwargs):
                args = command[1:]
                if args == ["--paths"]:
                    value = {"data": str(private / "data")}
                elif args == ["stop"]:
                    value = {"stopped": True}
                else:
                    if args == ["state"]:
                        perf = private / "data/perf"; perf.mkdir(exist_ok=True)
                        for name in ["client", "server"]:
                            (perf / (name + ".jsonl")).write_text(json.dumps({"kind": "run_start", "build_sha": head, "dev_tools": True}) + "\n")
                        result = {"build": {"binary": str(private / "bin/bus")}}
                    elif args[:2] == ["room", "create"]:
                        result = {"room_id": 1}
                    elif args[:2] == ["agent", "add"]:
                        result = {"agent_id": 10 if args[args.index("--name") + 1] == "fake-dialogs" else 11}
                    elif args[:2] == ["agent", "observe"]:
                        target = args[2]
                        text = ("TUI_DRIVER_DIALOGS_CLEARED:15" if completed[0] else "fake>") if target == "10" else ("No session yet" if target == "11" else ("OH_MY_ZSH_ANSWER:n" if completed[0] else "[Y/n]"))
                        result = {"text": text, "terminal_id": "term_" + target}
                    elif args[0] == "send":
                        is_driver = args[args.index("--to") + 1] == "11"
                        if is_driver:
                            completed[0] = True
                            (private / "data/fake-dialog-results.jsonl").write_text("".join(json.dumps({"id": name, "passed": True}) + "\n" for name in ids))
                        result = {"message_id": 21 if is_driver else 20}
                    elif args[:2] == ["message", "status"]:
                        text = "DRIVER_ACCEPTANCE_COMPLETE" if args[2] == "21" else "TUI_DRIVER_DIALOGS_CLEARED:15"
                        result = {"complete": True, "requests": [{"stage": "replied", "reply": {"text": text}}]}
                    else:
                        raise AssertionError("unexpected control call: " + repr(args))
                    value = {"ok": True, "result": result, "id": "mocked-outcome"}
                return subprocess.CompletedProcess(command, 0, json.dumps(value), "")
            class NoProcessSocket:
                def __init__(self, *args): pass
                def __enter__(self): return self
                def __exit__(self, *args): pass
                def settimeout(self, value): pass
                def connect(self, path): pass
                def sendall(self, value): self.request = json.loads(value)
                def makefile(self, mode):
                    result = {"root_pane": {"pane_id": "w2:p1"}} if self.request["method"] == "workspace.create" else {}
                    return io.BytesIO((json.dumps({"result": result}) + "\n").encode())
            case = module.DialogDriverAcceptance("test_real_agent_clears_all_fake_dialogs_and_shell_prompt")
            try:
                with patch.object(module, "REPO", repo), patch.object(module.Path, "home", return_value=root / "home"), patch.object(module.tempfile, "mkdtemp", side_effect=make_private), patch.object(module.os, "access", side_effect=lambda p, m: str(p).endswith("/.kimi-code/bin/kimi")), patch.object(module.subprocess, "check_output", side_effect=output), patch.object(module.subprocess, "run", side_effect=run), patch.object(module.socket, "socket", NoProcessSocket), patch.object(module.terminal_module, "Terminal", _NoProcessTerminal), patch.dict(os.environ, {"BUS_DIALOG_DRIVER_BINARY": str(binary), "BUS_DIALOG_DRIVER_SHA": head}, clear=True), contextlib.redirect_stdout(io.StringIO()):
                    with self.assertRaisesRegex(AssertionError, "driver route"):
                        case.test_real_agent_clears_all_fake_dialogs_and_shell_prompt()
            finally:
                case.doCleanups()


    def test_route_proof_requires_each_prompt_and_its_native_result(self):
        with tempfile.TemporaryDirectory() as directory:
            trace = Path(directory) / "client-trace.log"
            expected = ["one", "two"]
            records = [
                {"method": "agent.press", "request_id": request, "terminal_id": terminal, "audit_before": before, "ok": True, "selection_pending": False, "outcome": "sent", "key_count": 1, "text_written": False}
                for request, terminal, before in [("first", "fake", []), ("second", "fake", ["one"]), ("shell", "shell", [])]
            ]
            lines = [f'INFO bus.control.command{{control_request_id={row["request_id"]} method=agent.press}}: result event="bus.terminal.driver.result" terminal_id={row["terminal_id"]} outcome="sent" key_count=1 text_written=false' for row in records]
            setup = 'DEBUG api event="api.request.start" method=pane.send_input request_id=setup'
            def proof(active, body):
                trace.write_text("\n".join(body) + "\n")
                return support.driver_route_proof([trace], active, expected, "fake", "shell", {"setup"})
            valid = proof(records, lines + [setup])
            self.assertTrue(valid["passed"], valid)
            self.assertEqual(valid["dialogs"], {"one": "first", "two": "second"})
            missing = proof(records, [lines[0], lines[2]])
            self.assertFalse(missing["passed"])
            self.assertEqual(missing["missing_dialogs"], ["two"])
            repeated = proof([records[0], records[0], records[2]], lines)
            self.assertFalse(repeated["passed"], "repeated first-prompt input cannot prove the second prompt")
            wrong = [dict(row) for row in records]; wrong[1]["terminal_id"] = "other"
            self.assertFalse(proof(wrong, lines)["passed"])
            alternate = proof(records, lines + ['INFO bus.control.command{control_request_id=old method=agent.choose}: result'])
            self.assertFalse(alternate["passed"])
            self.assertEqual(alternate["alternate_input_methods"][0]["methods"], ["agent.choose"])

    def test_private_wrapper_records_receipts_without_answer_contents(self):
        from types import SimpleNamespace
        with tempfile.TemporaryDirectory() as directory:
            data = Path(directory)
            (data / "fake-dialog-results.jsonl").write_text('{"id":"previous","passed":true}\n')
            trace = data / "routes.jsonl"
            envelope = {"id": "press-proof", "ok": True, "result": {"observation": {"terminal_id": "fake"}, "outcome": "sent", "selection_pending": False, "keys": ["enter"], "text_written": True}}
            result = subprocess.CompletedProcess([], 0, json.dumps(envelope).encode(), b"")
            output = SimpleNamespace(stdout=SimpleNamespace(buffer=io.BytesIO()), stderr=SimpleNamespace(buffer=io.BytesIO()))
            with patch.object(support.subprocess, "run", return_value=result), patch.object(support, "sys", output):
                code = support.trace_command("never-executed", str(data), str(trace), ["agent", "press", "fake", "--text", "PRIVATE-ANSWER-NEVER-LOG", "--submit"])
            self.assertEqual(code, 0)
            self.assertEqual(json.loads(output.stdout.buffer.getvalue()), envelope)
            rows = support.route_records(trace)
            self.assertEqual(rows[0]["audit_before"], ["previous"])
            self.assertEqual(rows[0]["request_id"], "press-proof")
            self.assertTrue(rows[0]["text_written"])
            self.assertNotIn("PRIVATE-ANSWER-NEVER-LOG", trace.read_text())


@unittest.skipUnless(os.environ.get("BUS_DIALOG_DRIVER_BINARY") and
                     os.environ.get("BUS_DIALOG_DRIVER_SHA"), "opt-in real Kimi acceptance")
class DialogDriverAcceptance(unittest.TestCase):
    def test_real_agent_clears_all_fake_dialogs_and_shell_prompt(self):
        supplied = Path(os.environ["BUS_DIALOG_DRIVER_BINARY"])
        self.assertTrue(supplied.is_absolute(), "supply an absolute copied binary")
        binary = supplied.resolve(strict=True)
        self.assertNotEqual(binary, Path("/Users/dylanliu/work/bus/target/debug/bus"))
        sha = os.environ["BUS_DIALOG_DRIVER_SHA"]
        self.assertRegex(sha, r"^[0-9a-f]{40}$")
        self.assertEqual(sha, subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=REPO, text=True).strip())
        self.assertEqual(subprocess.check_output(
            ["git", "status", "--porcelain", "--untracked-files=all"], cwd=REPO, text=True), "",
            "acceptance requires the clean committed head")
        version = subprocess.check_output([str(binary), "--version"], text=True).strip()
        self.assertIn(sha, version)
        self.assertNotIn("-dirty", version)
        evidence = REPO / "temp/tui-driver/acceptance" / sha / ("run-" + secrets.token_hex(8))
        evidence.mkdir(parents=True, mode=0o700)
        evidence.chmod(0o700)
        root = Path(tempfile.mkdtemp(prefix="bus-dialog-driver-", dir="/private/tmp"))
        root.chmod(0o700)
        may_clean = [True]
        self.addCleanup(lambda: shutil.rmtree(root, ignore_errors=True) if may_clean[0] else None)
        for name in ["bin", "data", "project", "zsh"]:
            (root / name).mkdir(mode=0o700)
        copied = root / "bin/bus"
        shutil.copy2(binary, copied)
        digest = hashlib.sha256(copied.read_bytes()).hexdigest()
        self.assertEqual(hashlib.sha256(binary.read_bytes()).hexdigest(), digest)
        (root / "bin/bus-fake-agent").symlink_to(copied)
        kimi_bin = Path.home() / ".kimi-code/bin"
        self.assertTrue(os.access(kimi_bin / "kimi", os.X_OK), "approved real Kimi missing")
        path = f"{root / 'bin'}:{kimi_bin}:/usr/bin:/bin"
        for directory in path.split(":"):
            self.assertFalse(os.access(Path(directory) / "lark-cli", os.X_OK))
        env = {
            "HOME": str(Path.home()), "PATH": path, "SHELL": "/bin/zsh",
            "TERM": "xterm-256color", "LANG": "en_US.UTF-8",
            "ZDOTDIR": str(root / "zsh"), "BUS_DATA_DIR": str(root / "data"),
            "BUS_TUI_FAKE_AGENT": "1", "BUS_TUI_OWNER": secrets.token_hex(24),
        }
        trace_helper = root / "bin/driver-route.py"
        shutil.copy2(SUPPORT_PATH, trace_helper)
        route_path = root / "driver-routes.jsonl"
        wrapper = root / "bin/sbus"
        wrapper.write_text("#!/bin/sh\nexec env -i " + " ".join(
            shlex.quote(f"{key}={value}") for key, value in env.items()
        ) + " " + shlex.join([sys.executable, str(trace_helper), str(copied), str(root / "data"), str(route_path)]) + ' "$@"\n')
        wrapper.chmod(0o700)
        terminal = None
        stopped = False
        cleanup_errors = []
        traces = []
        routes = []
        setup_ids = set()

        def save(name, value):
            (evidence / f"{name}.json").write_text(json.dumps(value, indent=2) + "\n")
            return value

        def control(*args, optional=False):
            result = subprocess.run([str(wrapper), *args], cwd=root / "project", env=env,
                                    text=True, capture_output=True, timeout=30)
            if optional and result.returncode:
                if args == ("stop",):
                    save("stop-failure", {"exit_code": result.returncode, "stdout": result.stdout, "stderr": result.stderr})
                return None
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            value = json.loads(result.stdout)
            if args[0] not in ["--paths", "stop"]:
                self.assertTrue(value.get("ok"), value)
                return value["result"]
            return value

        def native(method, params):
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
                connection.settimeout(30)
                connection.connect(str(root / "data/herdr-config/sessions/bus/herdr.sock"))
                request_id = secrets.token_hex(8)
                if method == "pane.send_input":
                    setup_ids.add(request_id)
                connection.sendall((json.dumps({"id": request_id,
                                               "method": method, "params": params}) + "\n").encode())
                with connection.makefile("rb") as stream:
                    value = json.loads(stream.readline())
            self.assertNotIn("error", value, value)
            return value["result"]

        def wait_for(description, poll, timeout=90):
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                terminal.pump(0.2)
                value = poll()
                if value:
                    return value
            self.fail(f"Timed out: {description}; evidence={evidence}")

        def observe(target, pane=False):
            return control("agent", "observe", target, *(["--pane"] if pane else []))

        try:
            save("paths", paths := control("--paths"))
            self.assertEqual(Path(paths["data"]), root / "data")
            may_clean[0] = False
            terminal = terminal_module.Terminal([str(wrapper), "--dev", "--yolo"],
                                                root / "project", env)
            state = wait_for("private Bus startup", lambda: control("state", optional=True), 45)
            self.assertEqual(Path(state["build"]["binary"]).resolve(), copied.resolve())
            identity = []
            for name in ["client", "server"]:
                starts = [json.loads(line) for file in (root / "data").rglob(name + ".jsonl")
                          for line in file.read_text().splitlines()
                          if line.strip() and json.loads(line).get("kind") == "run_start"]
                self.assertTrue(starts, f"missing {name} build identity")
                self.assertEqual(starts[-1]["build_sha"], sha)
                self.assertTrue(starts[-1]["dev_tools"])
                identity.append({"process": name, **starts[-1]})
            save("identity", {"head": sha, "binary_sha256": digest, "runs": identity})
            # All following mutations target the verified private root.
            room = control("room", "create", "dialog-driver-acceptance")
            room_id = str(save("room", room)["room_id"])
            fake = str(save("fake-add", control("agent", "add", "--room", room_id,
                       "--name", "fake-dialogs", "--provider", "fake", "--pwd",
                       str(root / "project"), "--args", "--script " +
                       shlex.quote(str(FIXTURES / "suite.json"))))["agent_id"])
            wait_for("fake composer", lambda: "fake>" in observe(fake)["text"])
            fake_identity = save("fake-identity", observe(fake))
            fake_message = str(save("fake-send", control("send", "--room", room_id,
                               "--to", fake, "--text", "RUN_DIALOG_DRIVER_SUITE"))["message_id"])
            workspace = native("workspace.create", {"cwd": str(root / "project"), "focus": False})
            shell = save("shell-workspace", workspace)["root_pane"]["pane_id"]
            terminal.pump(1)
            native("pane.send_input", {"pane_id": shell, "text": "/bin/zsh -f " +
                   shlex.quote(str(FIXTURES / "oh-my-zsh.zsh")), "keys": ["Enter"]})
            wait_for("real-shell oh-my-zsh prompt", lambda: "[Y/n]" in observe(shell, True)["text"])
            shell_identity = save("shell-identity", observe(shell, True))
            driver = str(save("driver-add", control("agent", "add", "--room", room_id,
                         "--name", "real-driver", "--provider", "kimi", "--pwd",
                         str(root / "project")))["agent_id"])

            def ready_driver():
                screen = observe(driver)
                save("driver-startup", screen)
                if "Trust this folder?" in screen["text"]:
                    save("driver-trust", control("agent", "press", driver, "--select", "Trust and continue"))
                    return False
                return "No session yet" in screen["text"] or (
                    "context:" in screen["text"] and "thinking:" in screen["text"])

            wait_for("real Kimi composer after startup trust", ready_driver)
            answer_table = (FIXTURES / "README.md").read_text().split("| Prompt ID |")[1].split(
                "\n\nThe Codex")[0]
            task = f"""We are testing the TUI driver. Every dialog is from the fake harness in a
private scratch Bus and runs nothing real. Clear each with the tool using the
prescribed answers below, and report any dialog the tool could not read or answer.
Use this exact private command wrapper: {wrapper}
The fake target is agent {fake}. Observe it with `{wrapper} agent observe {fake}`.
Answer with `{wrapper} agent press {fake} --select 'complete row label'`,
`--key KEY`, or `--text 'answer' --submit`, as appropriate. --key 1 sends a number;
--text types without Enter unless --submit is present. Inspect every returned
screen before proceeding, especially a selection_pending or unknown outcome.
Clear all 15 sequential screens. Do not use old dialog/choose/answer commands or
other input mechanisms to answer them. Do not read the harness answer audit.
| Prompt ID |{answer_table}
Also use the SAME driver on real shell pane {shell}: `{wrapper} agent observe
{shell} --pane` then `{wrapper} agent press {shell} --pane --key n` to answer its
oh-my-zsh prompt without Enter. Confirm OH_MY_ZSH_ANSWER:n by observing that pane.
When all 15 are cleared and the shell confirms n, send a correlated reply to this
message with `{wrapper} send --reply-to MESSAGE_ID --as {driver} --text
'DRIVER_ACCEPTANCE_COMPLETE'`, using this actual Bus message ID. Otherwise reply
with the exact unreadable or unanswerable screen and tool problem."""
            save("driver-task", {"text": task})
            driver_message = str(save("driver-send", control("send", "--room", room_id,
                                 "--to", driver, "--text", task))["message_id"])
            seen = set()

            def complete():
                audits = audit_records(root / "data")
                save("audit", audits)
                self.assertTrue(all(row["passed"] for row in audits), audits)
                fake_screen = observe(fake)
                if fake_screen["text"] not in seen:
                    seen.add(fake_screen["text"])
                    with (evidence / "screens.jsonl").open("a") as stream:
                        stream.write(json.dumps(fake_screen) + "\n")
                shell_screen = save("shell-final", observe(shell, True))
                driver_status = save("driver-status", control("message", "status", driver_message))
                fake_status = save("fake-status", control("message", "status", fake_message))
                expected_ids = {action["id"] for action in json.loads(
                    (FIXTURES / "suite.json").read_text())["turns"][0]["actions"] if action["type"] == "screen"}
                return (len(audits) == 15 and {row["id"] for row in audits} == expected_ids
                        and "TUI_DRIVER_DIALOGS_CLEARED:15" in (reply_text(fake_status) + fake_screen["text"])
                        and "OH_MY_ZSH_ANSWER:n" in shell_screen["text"]
                        and driver_status.get("complete")
                        and all(request.get("stage") == "replied" for request in driver_status.get("requests", []))
                        and reply_text(driver_status).strip() == "DRIVER_ACCEPTANCE_COMPLETE")

            wait_for("real agent completes fake suite and shell", complete,
                     int(os.environ.get("BUS_DIALOG_DRIVER_TIMEOUT", "600")))
        finally:
            primary_error = sys.exc_info()[1]
            try:
                if terminal is not None:
                    try:
                        terminal.pump(0.2)
                        (evidence / "bus-terminal.ansi").write_bytes(terminal.output)
                    except Exception as error:
                        cleanup_errors.append("terminal evidence: " + str(error))
                    try:
                        stop = control("stop", optional=True)
                        stopped = bool(stop and stop.get("stopped"))
                        save("stop", stop or {"stopped": False, "reason": "private stop command failed"})
                    except Exception as error:
                        cleanup_errors.append("private stop: " + str(error))
                        save("stop", {"stopped": False, "reason": str(error)})
                    finally:
                        try:
                            terminal.close()
                        except Exception as error:
                            cleanup_errors.append("client close: " + str(error))
                else:
                    stopped = True
                try:
                    traces = support.collect_logs(root / "data", evidence)
                    routes = support.route_records(route_path)
                    save("route-trace", routes)
                except Exception as error:
                    cleanup_errors.append("log evidence: " + str(error))
                if stopped and not cleanup_errors:
                    may_clean[0] = True
                    shutil.rmtree(root)
                else:
                    may_clean[0] = False
                    save("retained-scratch", {"path": str(root), "reason": "private cleanup unconfirmed", "errors": cleanup_errors})
                save("cleanup", {"stopped": stopped, "scratch_removed": not root.exists(), "errors": cleanup_errors})
                if primary_error is not None or not stopped or cleanup_errors:
                    save("summary", {"head": sha, "result": "FAIL", "reason": str(primary_error) if primary_error is not None else "private cleanup unconfirmed"})
            finally:
                print(f"acceptance_evidence={evidence}", flush=True)
        self.assertTrue(stopped and not cleanup_errors, f"Private cleanup failed; evidence={evidence}")
        expected_ids = [action["id"] for action in json.loads((FIXTURES / "suite.json").read_text())["turns"][0]["actions"] if action["type"] == "screen"]
        proof = save("driver-route-proof", support.driver_route_proof(traces, routes, expected_ids,
                     fake_identity["terminal_id"], shell_identity["terminal_id"], setup_ids))
        save("summary", {"head": sha, "result": "PASS" if proof["passed"] else "FAIL", "dialogs": 15,
                         "real_provider": "kimi", "shell_answer": "n", "driver_routes": proof})
        self.assertTrue(proof["passed"], f"driver route evidence is incomplete: {proof}; evidence={evidence}")


if __name__ == "__main__":
    unittest.main()
