"""Opt-in unauthenticated Grok acceptance, adapted from inv-8259's repro.

Run GROK_LOGIN_BUS=/absolute/copied/bus python3 -m unittest \
    tools.tests.grok_login_native_test -v
Reads the real login home; never chooses login or changes provider config.
"""
import hashlib
import json
import os
from pathlib import Path
import struct
import subprocess
import tempfile
import time
import unittest
from unittest import mock

from tools.acceptance.harness_test import Bus


class RealHomeScratchBus(Bus):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.env.pop("TEST_BUS_PROTECTED_HOME", None)

    def prepare_skills(self):
        assert self.env["HOME"] == "/Users/dylanliu"
        assert self.data.is_relative_to(Path("/private/tmp"))
        assert self.data.stat().st_mode & 0o777 == 0o700


@unittest.skipUnless(os.environ.get("GROK_LOGIN_BUS"), "native scratch opt-in required")
class GrokLoginNativeTest(unittest.TestCase):
    def test_grok_login_is_blocked_and_work_stays_queued(self):
        binary = Path(os.environ["GROK_LOGIN_BUS"]).resolve()
        assert binary.is_file() and binary != Path.cwd() / "target/debug/bus"
        root = Path(tempfile.mkdtemp(prefix="grok-login-", dir="/private/tmp"))
        root.chmod(0o700)
        for name in ["bin", "data", "project"]:
            (root / name).mkdir(mode=0o700)
        (root / "bin/bus").symlink_to(binary)
        (root / "bin/grok").symlink_to("/Users/dylanliu/.grok/bin/grok")
        for name in ["open", "osascript", "afplay", "terminal-notifier"]:
            shim = root / "bin" / name
            shim.write_text("#!/bin/sh\nexit 0\n")
            shim.chmod(0o700)
        env = {"HOME": "/Users/dylanliu", "SHELL": "/bin/bash",
               "PATH": f"{root}/bin:/usr/bin:/bin", "TERM": "xterm-256color",
               "BUS_DATA_DIR": str(root / "data")}
        if os.environ.get("GROK_LOGIN_FORCE_NETWORK_ERROR") == "1":
            env.update(HTTPS_PROXY="http://127.0.0.1:9", https_proxy="http://127.0.0.1:9",
                       ALL_PROXY="http://127.0.0.1:9")
        for folder in env["PATH"].split(":"):
            assert not os.access(Path(folder) / "lark-cli", os.X_OK)
        subprocess.run(["/usr/bin/git", "init", "-q", str(root / "project")],
                       env=env, check=True)
        report = {"root": str(root), "binary": str(binary),
                  "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                  "commands": []}
        columns = int(os.environ.get("GROK_LOGIN_TUI_COLUMNS", "160"))
        rows = int(os.environ.get("GROK_LOGIN_TUI_ROWS", "45"))
        report["requested_viewport"] = {"columns": columns, "rows": rows}
        print("SCRATCH_ROOT=" + str(root), flush=True)
        bus = RealHomeScratchBus(binary, root, env, workspace=root / "project")

        def cli(*args):
            result = bus.cli(*map(str, args))
            report["commands"].append({"argv": list(map(str, args)), "result": result})
            (root / "report.json").write_text(json.dumps(report, indent=2))
            return result

        try:
            report["paths"] = json.loads(subprocess.check_output(
                [str(binary), "--paths"], env=env, text=True))
            self.assertEqual(report["paths"]["data"], str(root / "data"))
            report["identity"] = subprocess.check_output(
                [str(binary), "--identity"], env=env, text=True)
            report["provider_version"] = subprocess.check_output(
                [str(root / "bin/grok"), "--version"], env=env, text=True)
            original_pack = struct.pack

            def viewport_pack(fmt, *values):
                if fmt == "HHHH" and values == (45, 160, 0, 0):
                    return original_pack(fmt, rows, columns, 0, 0)
                return original_pack(fmt, *values)

            with mock.patch("tools.acceptance.harness_test.struct.pack", side_effect=viewport_pack):
                bus.start()
            room = cli("room", "create", "login-test")["room_id"]
            agent = cli("agent", "add", "--room", room, "--name", "login-test",
                        "--provider", "grok", "--pwd", root / "project",
                        "--consent-hooks")["agent_id"]
            deadline = time.monotonic() + 50
            while time.monotonic() < deadline:
                observation = cli("agent", "observe", agent)
                row = next(a for a in cli("diagnostics")["agents"] if a["agent_id"] == agent)
                if "Login with Grok" in observation["text"] and row["status"] == "blocked":
                    break
                time.sleep(1)
            report["visible"] = cli("agent", "read", agent, "--source", "visible")
            report["native"] = bus.native_agent(f"bus-r{room}-a{agent}")
            self.assertEqual(row["status"], "blocked", "login did not become blocked; see " + str(root))
            self.assertIn("Login with Grok", observation["text"], "no native login controls; see " + str(root))
            self.assertEqual(observation["terminal_id"], report["native"]["terminal_id"])
            self.assertNotEqual(row.get("reason"), "session_hook_missing")
            message = cli("send", "--room", room, "--to", agent, "--text",
                          "Reply with GROK_LOGIN_QUEUED only.")["message_id"]
            # Cross the former session_hook_missing abandonment budget (120s).
            until = time.monotonic() + 125
            while True:
                status = cli("message", "status", message)
                self.assertFalse(status["complete"], str(status))
                self.assertEqual(status["requests"][0]["stage"], "queued")
                self.assertEqual(status["requests"][0]["status"], "blocked")
                self.assertNotEqual(status["requests"][0]["reason"], "session_hook_missing")
                if time.monotonic() >= until:
                    break
                time.sleep(2)
            report["final_status"] = status
            report["final_visible"] = cli("agent", "read", agent, "--source", "visible")
            report["final_observation"] = cli("agent", "observe", agent)
            self.assertIn("Login with Grok", report["final_observation"]["text"])
            report["history"] = cli("history", "--room", room, "--limit", 30)
        finally:
            report["cleanup"] = bus.stop()
            (root / "report.json").write_text(json.dumps(report, indent=2))
