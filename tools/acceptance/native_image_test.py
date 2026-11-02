"""Capture a real Codex image+reply command against a loopback-only HTTP endpoint.

Manual acceptance: no build, account credentials, model response, or live Bus.
Importing this module never launches Bus or Codex.
"""
import argparse
import hashlib
import http.server
import json
import os
from pathlib import Path
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
import zlib

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))
from tools.acceptance.harness_test import Bus, pids_holding  # noqa: E402


def until(predicate, timeout=40):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = predicate()
        if result:
            return result
        time.sleep(0.05)
    raise AssertionError("native capture predicate did not arrive")


def send_images(bus, images):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(15)
        stream.connect(str(bus.data / "control.sock"))
        # Multiple file attachments render on one space-separated line; use
        # separate quoted text lines to exercise independent image pastes.
        request = {"id": "native-image", "method": "message.send", "params": {
            "room": "bus", "to": ["image-worker"],
            "text": "\n".join(f'"{image}"' for image in images) if len(images) > 1 else "",
            "files": [str(image) for image in images] if len(images) == 1 else []}}
        stream.sendall(json.dumps(request).encode() + b"\n")
        receipt = json.loads(stream.makefile().readline())
    assert receipt["ok"], receipt
    return receipt["result"]


def image_png():
    def chunk(name, data):
        return (struct.pack("!I", len(data)) + name + data
                + struct.pack("!I", zlib.crc32(name + data)))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack("!2I5B", 2, 2, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress((b"\0" + b"\0\0\xff" * 2) * 2)) + chunk(b"IEND", b""))


def user_messages(captures):
    return [item for capture in captures for item in capture["body"].get("input", [])
            if isinstance(item, dict) and item.get("role") == "user"]


def contains_command(message, command):
    return any(part.get("type") == "input_text" and command in part.get("text", "")
               for part in message.get("content", []))


def isolated_environment(binary, codex, root, port):
    for name in ("home", "bin", "tmp", "config", "data-home", "cache", "work", "codex-home"):
        (root / name).mkdir()
    for name in ("claude", "cursor-agent", "open", "osascript", "terminal-notifier", "afplay"):
        (root / "bin" / name).symlink_to(binary)
    (root / "bin/codex").symlink_to(codex)
    config = f'''model="gpt-5.4"
model_provider="capture"
check_for_update_on_startup=false
[model_providers.capture]
name="local-capture"
base_url="http://127.0.0.1:{port}/v1"
wire_api="responses"
requires_openai_auth=false
request_max_retries=0
stream_max_retries=0
[projects.{json.dumps(str(root / "work"))}]
trust_level="trusted"
[analytics]
enabled=false
[feedback]
enabled=false
'''
    (root / "codex-home/config.toml").write_text(config)
    env = {"PATH": f"{root}/bin:/usr/bin:/bin:/usr/sbin:/sbin", "HOME": str(root / "home"),
           "SHELL": "/bin/sh", "TERM": "xterm-256color", "TMPDIR": str(root / "tmp"),
           "XDG_CONFIG_HOME": str(root / "config"), "XDG_DATA_HOME": str(root / "data-home"),
           "XDG_CACHE_HOME": str(root / "cache"), "CODEX_HOME": str(root / "codex-home"),
           "BUS_DATA_DIR": str(root / "data"), "BUS_TUI_FAKE_AGENT": "1",
           "BUS_TUI_EFFECTS": str(root / "effects.jsonl")}
    return env


def scenario(binary, codex, root, kinds):
    captures = []
    lock = threading.Lock()

    class Endpoint(http.server.BaseHTTPRequestHandler):
        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
            with lock:
                captures.append({"path": self.path, "body": body})
            # The real provider serializes its request, but no model answers it.
            self.send_response(400)
            self.end_headers()
            self.wfile.write(b'{"error":{"message":"local capture complete","type":"invalid_request_error"}}')

        def do_GET(self):
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b'{"data":[{"id":"gpt-5.4"}]}')

        def log_message(self, *args):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Endpoint)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    evidence = {"result": "FAIL", "binary": str(binary), "codex": str(codex),
                "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                "run_dir": str(root), "endpoint": f"http://127.0.0.1:{server.server_port}/v1"}
    bus = None
    try:
        env = isolated_environment(binary, codex, root, server.server_port)
        evidence["codex_version"] = subprocess.check_output([str(codex), "--version"], env=env, text=True).strip()
        bus = Bus(binary, root, env, workspace=root / "work")
        bus.start()
        bus.cli("agent", "add", "--room", "bus", "--name", "image-worker", "--provider", "codex",
                "--pwd", str(root / "work"), "--consent-hooks")

        hook_reviewed = False

        def ready():
            nonlocal hook_reviewed
            agent = next(a for a in bus.cli("state")["agents"] if a["name"] == "image-worker")
            if agent["status"] == "idle":
                native = bus.native_agent(agent["runtime_identity"]["pane_id"])
                if native and native.get("native_ready") and native.get("interactive_ready"):
                    return agent
            screen = bus.cli("agent", "observe", "image-worker")
            if "Hooks need review" in screen.get("text", "") and not hook_reviewed:
                evidence.setdefault("hook_reviews", []).append(screen)
                selected = bus.cli("agent", "press", "image-worker", "--select", "Trust all and continue",
                                   "--terminal-id", screen["terminal_id"])
                evidence.setdefault("hook_review_inputs", []).append(selected)
                assert selected.get("outcome") == "selected" and not selected.get("selection_pending"), selected
                hook_reviewed = True
            return None

        agent = until(ready)
        images = [root / f"image-{index}.png" for index in range(len(kinds))]
        for image, kind in zip(images, kinds):
            image.write_bytes(image_png() if kind == "valid" else b"not a decodable image\n")
        evidence["image_kinds"] = kinds
        receipt = send_images(bus, images)
        evidence["receipt"] = receipt
        command = f"bus send --reply-to {receipt['message_id']} --as {agent['id']} --text 'YOUR ANSWER'"
        evidence["expected_command"] = command

        def captured():
            with lock:
                snapshot = list(captures)
            return snapshot if any(contains_command(message, command)
                                   for message in user_messages(snapshot)) else None

        evidence["captures"] = until(captured)
        matching = [message for message in user_messages(evidence["captures"])
                    if contains_command(message, command)]
        def intact(message):
            content = message.get("content", [])
            text = "".join(part.get("text", "") for part in content if part.get("type") == "input_text")
            return (sum(part.get("type") == "input_image" for part in content) == kinds.count("valid")
                    and all(f'"{image}"' in text for image, kind in zip(images, kinds) if kind == "invalid"))

        assert any(intact(message) for message in matching), "correlated user message lost an image or literal fallback"
        evidence["matched_user_messages"] = matching
        evidence["screen"] = bus.cli("agent", "read", "image-worker", "--source", "visible")
        def bound():
            status = bus.cli("message", "status", str(receipt["message_id"]))
            evidence["status"] = status
            return status if status["requests"] and all(request["start_bound"] for request in status["requests"]) else None

        evidence["status"] = until(bound)
        evidence["result"] = "PASS"
    except Exception as error:
        evidence["error"] = f"{type(error).__name__}: {error}"
        try:
            evidence["screen"] = bus.cli("agent", "read", "image-worker", "--source", "visible")
        except Exception as screen_error:
            evidence["screen_error"] = str(screen_error)
    finally:
        with lock:
            evidence["captures"] = list(captures)
        evidence["callbacks"] = {}
        if bus is not None:
            for path in bus.data.rglob("*.json"):
                if "callbacks" not in path.relative_to(bus.data).parts:
                    continue
                try:
                    evidence["callbacks"][str(path.relative_to(bus.data))] = json.loads(path.read_text())
                except (OSError, ValueError) as error:
                    evidence["callbacks"][str(path.relative_to(bus.data))] = {"read_error": str(error)}
        try:
            evidence["cleanup"] = bus.stop() if bus is not None else {}
        except Exception as error:
            evidence["cleanup"] = {"error": str(error)}
            evidence["result"] = "FAIL"
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        leftovers = sorted(pids_holding([root]))
        evidence["cleanup"]["leftover"] = leftovers
        if leftovers:
            evidence["result"] = "FAIL"
        else:
            shutil.rmtree(root)
        evidence["cleanup"]["root_removed"] = not root.exists()
    return evidence


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--codex", required=True, type=Path, help="Native Codex executable, not an account wrapper")
    parser.add_argument("--run-dir", type=Path, help="New short direct child of /private/tmp (or /tmp)")
    parser.add_argument("--report", type=Path)
    parser.add_argument("--images", choices=("valid", "invalid", "valid,invalid", "invalid,valid"),
                        default="valid", help="Per-paste native decode outcomes to verify")
    args = parser.parse_args()
    binary, codex = args.binary.resolve(strict=True), args.codex.resolve(strict=True)
    for path in (binary, codex):
        assert path.is_file() and os.access(path, os.X_OK), f"not executable: {path}"
    git_env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    entries = subprocess.check_output(["git", "-C", str(ROOT), "worktree", "list", "--porcelain", "-z"], env=git_env)
    main_root = Path(os.fsdecode(entries.split(b"\0", 1)[0].removeprefix(b"worktree ")))
    live = main_root / "target/debug/bus"
    assert not (binary == live.resolve() or live.exists() and binary.samefile(live)), "live Bus binary forbidden"
    temporary = Path("/private/tmp" if sys.platform == "darwin" else "/tmp").resolve()
    if args.run_dir:
        root = args.run_dir.resolve()
        assert root.parent == temporary and len(root.name) <= 20, "run directory must be a short new temp root"
        root.mkdir(mode=0o700)
    else:
        root = Path(tempfile.mkdtemp(prefix="bni-", dir=temporary))
    report = (args.report or ROOT / "temp/acceptance" / f"native-image-{root.name}.json").resolve()
    assert not report.is_relative_to(root), "report must survive scratch cleanup"
    evidence = scenario(binary, codex, root, args.images.split(","))
    evidence["source_head"] = subprocess.check_output(["git", "-C", str(ROOT), "rev-parse", "HEAD"], env=git_env, text=True).strip()
    report.parent.mkdir(parents=True, exist_ok=True)
    report.write_text(json.dumps(evidence, indent=2))
    print(f"{evidence['result']} {report}")
    return 0 if evidence["result"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
