#!/usr/bin/env python3
"""Opt-in real-provider Bus integration driver. Never fabricates callbacks.

Run with --allow-live-models, then provide JSON commands on stdin. Setup/trust
menus are deliberately interactive: inspect `frame` before sending any choice.
Commands: frame, state, send(text), paste(text), click(label, sidebar), add
(provider, name), select(names), case(names), verify(token), capture(name), stop.
All prompts go through the actual @ checkbox picker and composer Enter path.
Only the disposable Bus session is stopped. Existing credentials remain with
the installed CLIs; this driver never reads or copies credential files.
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
import struct
import subprocess
import sys
import tempfile
import termios
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]
ROWS, COLS, SIDEBAR = 40, 120, 28
# Exact executable plus steps from the Add form's default Codex selection.
PROVIDERS = {"codex": ("codex", 0), "claude": ("claude", 1), "cursor": ("cursor-agent", 2)}


def terminal_frame(raw):
    """Read Herdr's absolute-cell capture; this is not the app's renderer."""
    screen = [[" "] * COLS for _ in range(ROWS)]
    x = y = i = 0
    text = raw.decode("utf-8", errors="replace")
    while i < len(text):
        c = text[i]
        if c == "\x1b":
            if text[i:i + 2] == "\x1b[":
                match = re.match(r"\x1b\[([0-?]*)([ -/]*)([@-~])", text[i:])
                if not match:
                    break
                params, _, final = match.groups()
                values = [int(v) if v.isdigit() else 0 for v in params.split(";")]
                n = values[0] or 1
                if final in "Hf":
                    y = max(0, (values[0] or 1) - 1)
                    x = max(0, ((values[1] if len(values) > 1 else 1) or 1) - 1)
                elif final == "G": x = n - 1
                elif final == "A": y = max(0, y - n)
                elif final == "B": y = min(ROWS - 1, y + n)
                elif final == "C": x = min(COLS - 1, x + n)
                elif final == "D": x = max(0, x - n)
                elif final == "J" and values[0] in (2, 3):
                    screen = [[" "] * COLS for _ in range(ROWS)]
                elif final == "K" and y < ROWS:
                    start = 0 if values[0] in (1, 2) else x
                    end = x + 1 if values[0] == 1 else COLS
                    for column in range(start, min(end, COLS)):
                        screen[y][column] = " "
                i += len(match[0])
                continue
            if text[i:i + 2] in ("\x1b]", "\x1bP", "\x1b_", "\x1b^"):
                end = re.search("\x07|\x1b\\\\", text[i + 2:])
                if not end:
                    break
                i += 2 + end.end()
                continue
            i += 2
            continue
        if c == "\r": x = 0
        elif c == "\n": y = min(ROWS - 1, y + 1)
        elif c == "\b": x = max(0, x - 1)
        elif ord(c) >= 32:
            if y < ROWS and x < COLS:
                screen[y][x] = c
            x += 1
        i += 1
    return "\n".join("".join(row).rstrip() for row in screen) + "\n"


def assert_reply_cards(frame, names, token):
    lines = [line[SIDEBAR:].strip() for line in frame.splitlines()]
    for name in names:
        assert any(
            lines[index].startswith(name + "  ")
            and lines[index + 1] == token
            and lines[index + 2] == "Quote"
            for index in range(len(lines) - 2)
        ), f"Final reply card for {name} is not visible in room"


class LiveBus:
    def __init__(self):
        self.data = Path(tempfile.mkdtemp(prefix="bus-live-", dir="/tmp")).resolve()
        self.artifacts = ROOT / "temp/artifacts" / self.data.name
        self.artifacts.mkdir(parents=True)
        self.capture = bytearray()
        self.cases = {}
        self.versions = {}
        for provider, (command, _) in PROVIDERS.items():
            executable = shutil.which(command)
            if not executable:
                raise RuntimeError(f"Missing real {command} executable")
            result = subprocess.run([executable, "--version"], capture_output=True,
                                    text=True, check=True, timeout=10)
            self.versions[provider] = {"executable": executable, "version": result.stdout.strip()}
        self.env = dict(os.environ, BUS_DATA_DIR=str(self.data), TERM="xterm-256color")
        for key in ("HERDR_ENV", "HERDR_SOCKET_PATH", "HERDR_CLIENT_SOCKET_PATH", "HERDR_CONFIG_PATH"):
            self.env.pop(key, None)
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
        self.child = subprocess.Popen([str(ROOT / "bus")], stdin=slave, stdout=slave,
                                     stderr=slave, env=self.env, start_new_session=True)
        os.close(slave)
        self.pump(3)
        if self.child.poll() is not None:
            raise RuntimeError(self.frame())
        self.send("\x12")
        self.send("live-roundtrip\r")
        self.save("initial")

    def pump(self, seconds=0.4):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            ready = select.select([self.master], [], [], min(0.05, max(0, deadline - time.monotonic())))[0]
            if ready:
                try:
                    chunk = os.read(self.master, 65536)
                except OSError:
                    break
                if not chunk:
                    break
                self.capture.extend(chunk)

    def send(self, text):
        os.write(self.master, text.encode())
        self.pump()

    def paste(self, text):
        self.send("\x1b[200~" + text + "\x1b[201~")

    def frame(self):
        return terminal_frame(self.capture)

    def state(self):
        return json.loads((self.data / "state.json").read_text())["state"]

    def room(self):
        return next(room for room in self.state()["rooms"].values() if room["name"] == "live-roundtrip")

    def click(self, label, sidebar=False):
        for row, line in enumerate(self.frame().splitlines()):
            if label in ("Add", "Cancel") and "Cancel" not in line:
                continue
            region = line[:SIDEBAR] if sidebar else line[SIDEBAR:]
            match = re.search(r"(?<!\w)" + re.escape(label) + r"(?!\w)", region)
            if match:
                col = match.start() + (0 if sidebar else SIDEBAR)
                self.send(f"\x1b[<0;{col + 1};{row + 1}M\x1b[<0;{col + 1};{row + 1}m")
                return
        raise AssertionError(f"No visible {label!r}\n{self.frame()}")

    def save(self, name):
        if not re.fullmatch(r"[a-zA-Z0-9_-]+", name):
            raise ValueError("Invalid capture name")
        (self.artifacts / f"{name}.txt").write_text(self.frame())
        (self.artifacts / "terminal.ansi").write_bytes(self.capture)
        (self.artifacts / "evidence.json").write_text(json.dumps({
            "real_providers": True, "data_root": str(self.data), "versions": self.versions,
            "cases": self.cases, "state": self.state(),
        }, indent=2) + "\n")

    def add(self, provider, name):
        if provider not in PROVIDERS or not re.fullmatch(r"[a-zA-Z0-9_-]+", name):
            raise ValueError("Expected claude/codex/cursor and simple test alias")
        cwd = self.data / (name + "-project")
        cwd.mkdir(exist_ok=False)
        subprocess.run(["git", "init", "-q", str(cwd)], check=True)
        self.send("\x0e")
        self.paste(name)
        self.send("\t")
        for _ in range(PROVIDERS[provider][1]):
            self.send("\x1b[B")
        self.send("\t")
        self.send("\x1b[H\x1b[3~\x1b[3~")  # default PWD is ~/
        self.paste(str(cwd))
        self.click("Add")
        self.save("add-" + name)

    def select_agents(self, names):
        self.send("\x1b[17~")  # F6: real room view
        agents = {a["name"]: a["id"] for a in self.state()["agents"].values()}
        wanted = {agents[name] for name in names}
        self.send("@")
        selected = set(self.room()["draft"]["recipient_ids"])
        for name, identity in agents.items():
            if (identity in selected) != (identity in wanted):
                # The same name can be visible on a reply card behind the menu.
                self.click(("[x] " if identity in selected else "[ ] ") + name)
        self.save("at-selection-" + str(len(self.cases) + 1))
        self.send("\x1b")
        assert set(self.room()["draft"]["recipient_ids"]) == wanted, "@ selection did not persist"

    def start_case(self, names):
        assert not self.room()["draft"]["text"], "Refuse to overwrite a draft"
        self.select_agents(names)
        token = "BUS_LIVE_" + uuid.uuid4().hex[:12].upper()
        prompt = f"Reply with exactly {token} and nothing else. Do not use tools or modify files."
        previous = set(self.state()["requests"])
        self.paste(prompt)
        self.save("composer-" + token)
        self.send("\r")
        self.pump(1)
        requests = {key: value for key, value in self.state()["requests"].items() if key not in previous}
        assert len(requests) == len(names), "Enter did not create exactly one request per selected agent"
        assert all(r["prompt"]["text"] == prompt for r in requests.values())
        self.cases[token] = {"names": names, "prompt": prompt, "request_ids": list(requests), "passed": False}
        self.save("sent-" + token)
        return token

    def verify(self, token):
        case = self.cases[token]
        state = self.state()
        room = self.room()
        replies = {}
        for identity in case["request_ids"]:
            request = state["requests"][identity]
            assert request["phase"] == "completed", f"request {identity}: {request['phase']}"
            reply = room["latest_replies"][str(request["agent_id"])]
            assert reply["request_id"] == request["id"], "Reply belongs to a different request"
            assert reply["text"].strip() == token, f"Unexpected real response: {reply['text']!r}"
            assert request["trusted_start_bound"] and request["provider_session_id"] and request["provider_turn_id"]
            replies[state["agents"][str(request["agent_id"])] ["name"]] = reply
        self.send("\x1b[17~")
        assert_reply_cards(self.frame(), case["names"], token)
        case.update(passed=True, replies=replies)
        self.save("reply-" + token)
        return replies

    def stop(self):
        self.save("last")
        if self.child.poll() is None:
            self.send("\x11")
            self.pump(1)
        result = subprocess.run([str(ROOT / "target/debug/herdr"), "session", "stop", "bus"],
                                env=self.env, capture_output=True, text=True, timeout=20)
        self.pump()
        if self.child.poll() is None:
            self.child.terminate()
            self.child.wait(timeout=5)
        os.close(self.master)
        return {"isolated_server_stop": result.returncode, "artifacts": str(self.artifacts)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--allow-live-models", action="store_true", required=True,
                        help="Explicitly authorize short real CLI model calls")
    parser.parse_args()
    bus = LiveBus()
    print(json.dumps({"ready": True, "data_root": str(bus.data), "artifacts": str(bus.artifacts),
                      "versions": bus.versions}), flush=True)
    try:
        pending = b""
        while True:
            # Keep draining the real UI even while the operator inspects results;
            # otherwise a provider could stall behind terminal output backpressure.
            bus.pump(0.05)
            if b"\n" not in pending:
                if not select.select([sys.stdin], [], [], 0.05)[0]:
                    continue
                chunk = os.read(sys.stdin.fileno(), 65536)
                if not chunk:
                    break
                pending += chunk
                continue
            line, pending = pending.split(b"\n", 1)
            try:
                command = json.loads(line)
                op = command["op"]
                bus.pump(0.1)
                if op == "stop":
                    break
                if op == "send": bus.send(command["text"])
                elif op == "paste": bus.paste(command["text"])
                elif op == "click": bus.click(command["label"], command.get("sidebar", False))
                elif op == "add": bus.add(command["provider"], command["name"])
                elif op == "select": bus.select_agents(command["names"])
                elif op == "case": print(json.dumps({"token": bus.start_case(command["names"])}), flush=True)
                elif op == "verify": print(json.dumps({"verified": bus.verify(command["token"])}), flush=True)
                elif op == "capture": bus.save(command["name"])
                elif op == "state": print(json.dumps(bus.state(), indent=2), flush=True)
                elif op != "frame": raise ValueError("Unknown operation")
                print(bus.frame(), flush=True)
            except (KeyError, ValueError, AssertionError, OSError) as error:
                print(json.dumps({"error": str(error)}), flush=True)
    finally:
        print(json.dumps(bus.stop()), flush=True)


if __name__ == "__main__":
    main()
