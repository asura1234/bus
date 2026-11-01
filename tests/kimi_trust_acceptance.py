"""Opt-in real Kimi trust acceptance adapted from inv-8259's failing probe.

BUS_KIMI_TRUST_BINARY=/absolute/bus BUS_KIMI_TRUST_SHA=<full-sha> \
    python3 tests/kimi_trust_acceptance.py -v
Uses real HOME, a fresh private Bus and project. Only Kimi's native trust answer
may add one throwaway project entry to the real store; credentials are never copied.
"""

import fcntl
import hashlib
import json
import os
from pathlib import Path
import pty
import select
import shutil
import struct
import subprocess
import tempfile
import termios
import time
import unittest


class Terminal:
    def __init__(self, argv, cwd, env):
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 46, 150, 0, 0))

        def session():
            os.setsid()
            fcntl.ioctl(slave, termios.TIOCSCTTY, 0)

        self.process = subprocess.Popen(
            argv, cwd=cwd, env=env, stdin=slave, stdout=slave, stderr=slave,
            preexec_fn=session,
        )
        os.close(slave)
        self.output = bytearray()

    def pump(self, seconds=0.2):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if not select.select([self.master], [], [], 0.1)[0]:
                continue
            try:
                data = os.read(self.master, 65536)
            except OSError:
                return
            if not data:
                return
            self.output.extend(data)
            if b"\x1b[6n" in data:
                os.write(self.master, b"\x1b[1;1R")
            if b"\x1b[c" in data or b"\x1b[0c" in data:
                os.write(self.master, b"\x1b[?1;2c")

    def close(self):
        os.close(self.master)
        if self.process.poll() is None:
            try:
                self.process.terminate()
                self.process.wait(timeout=5)
            except (subprocess.TimeoutExpired, PermissionError, ProcessLookupError):
                try:
                    self.process.kill()
                    self.process.wait(timeout=5)
                except (subprocess.TimeoutExpired, PermissionError, ProcessLookupError):
                    pass


def private_root():
    root = Path(tempfile.mkdtemp(prefix="bus-inv-trust-", dir="/private/tmp"))
    root.chmod(0o700)
    return root


def isolated_environment(root):
    # Construct from an allowlist: no live BUS_SOCKET or inherited HERDR_*.
    return {
        "HOME": str(Path.home()),
        "PATH": f"{root / 'bin'}:/usr/bin:/bin",
        "TERM": "xterm-256color",
        "LANG": "en_US.UTF-8",
        "SHELL": "/bin/zsh",
        "BUS_DATA_DIR": str(root / "data"),
    }


def trust_snapshot():
    root = Path.home() / ".kimi-code/workspace-trust"
    return {
        p.name: hashlib.sha256(p.read_bytes()).hexdigest()
        for p in root.glob("*") if p.is_file()
    }


class KimiFolderTrustAcceptance(unittest.TestCase):
    def test_trust_answer_persists_to_a_second_fresh_agent(self):
        binary = Path(os.environ["BUS_KIMI_TRUST_BINARY"]).resolve()
        self.assertNotEqual(binary, Path("/Users/dylanliu/work/bus/target/debug/bus"))
        expected_sha = os.environ["BUS_KIMI_TRUST_SHA"]
        self.assertRegex(expected_sha, r"^[0-9a-f]{40}$")
        root = private_root()
        print(f"scratch_evidence={root}", flush=True)
        for name in ["data", "project", "bin"]:
            (root / name).mkdir(mode=0o700)
        shutil.copy2(binary, root / "bin/bus")
        self.assertEqual(hashlib.sha256(binary.read_bytes()).digest(),
                         hashlib.sha256((root / "bin/bus").read_bytes()).digest())
        for path in [root / "bin", Path("/usr/bin"), Path("/bin")]:
            self.assertFalse(os.access(path / "lark-cli", os.X_OK))
        env = isolated_environment(root)
        self.assertEqual(env["HOME"], "/Users/dylanliu")
        before = trust_snapshot()
        terminal = None

        def control(*args):
            result = subprocess.run(
                [str(root / "bin/bus"), *args], cwd=root / "project", env=env,
                text=True, capture_output=True, timeout=30,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            value = json.loads(result.stdout)
            if args[0] not in ["--paths", "stop"]:
                self.assertTrue(value["ok"], value)
            return value

        def save(name, value):
            (root / (name + ".json")).write_text(json.dumps(value, indent=2))
            return value

        def observe(agent, expect_trust):
            deadline = time.monotonic() + 45
            while time.monotonic() < deadline:
                terminal.pump()
                dialog = control("agent", "dialog", agent)
                visible = control("agent", "read", agent, "--source", "visible")
                text = json.dumps(visible)
                if "Trust this folder?" in json.dumps(dialog) or "No session yet" in text:
                    save(agent + "-dialog", dialog)
                    save(agent + "-visible", visible)
                    self.assertEqual("Trust this folder?" in json.dumps(dialog), expect_trust,
                                     f"agent={agent}; evidence={root}")
                    return dialog["result"]
            self.fail(f"No trust chooser or native composer observed; evidence={root}")

        try:
            paths = save("paths", control("--paths"))
            self.assertIn(str(root / "data"), json.dumps(paths))
            terminal = Terminal([str(root / "bin/bus"), "--dev"], root / "project", env)
            deadline = time.monotonic() + 40
            while True:
                terminal.pump()
                state = subprocess.run([str(root / "bin/bus"), "state"], cwd=root,
                                       env=env, text=True, capture_output=True, timeout=5)
                if state.returncode == 0:
                    break
                self.assertLess(time.monotonic(), deadline, "scratch Bus did not start")
            identity = []
            for name in ["client", "server"]:
                records = [json.loads(line) for path in (root / "data").rglob(name + ".jsonl")
                           for line in path.read_text().splitlines()]
                starts = [r for r in records if r.get("kind") == "run_start"]
                self.assertTrue(starts, (name, records[:1]))
                self.assertEqual(starts[-1]["build_sha"], expected_sha)
                identity.append(starts[-1])
            save("build-identity", identity)
            room = save("room", control("room", "create", "trust-acceptance"))
            room_id = str(room["result"]["room_id"])

            def add(name):
                result = save(name + "-add", control("agent", "add", "--room", room_id,
                              "--name", name, "--provider", "kimi", "--pwd", str(root / "project")))
                return str(result["result"]["agent_id"])

            first = add("first-kimi")
            dialog = observe(first, True)
            option = next(o["number"] for o in dialog["dialog"]["options"]
                          if "Trust and continue" in o["label"])
            save("choose", control("agent", "choose", first, "--option", str(option),
                                   "--fingerprint", dialog["fingerprint"]))
            observe(first, False)
            second = add("second-kimi")
            observe(second, False)
            after = trust_snapshot()
            added = sorted(after.keys() - before.keys())
            self.assertEqual(len(added), 1, (added, root))
            self.assertTrue(all(after.get(key) == value for key, value in before.items()))
            record = json.loads((Path.home() / ".kimi-code/workspace-trust" / added[0]).read_text())
            self.assertEqual(record["root"], str(root / "project"))
            save("summary", {"head": expected_sha, "first_asks": True, "second_asks": False,
                             "trust_key": added[0], "trust_record": record})
            print(f"trust_key={added[0]}", flush=True)
        finally:
            if terminal is not None:
                stopped = save("stop", control("stop"))
                self.assertTrue(stopped["stopped"], stopped)
                terminal.close()


if __name__ == "__main__":
    unittest.main()
