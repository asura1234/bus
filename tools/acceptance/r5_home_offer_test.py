import importlib.util
import os
import signal
from pathlib import Path

spec = importlib.util.spec_from_file_location('harness', Path(__file__).with_name('harness_test.py'))
harness = importlib.util.module_from_spec(spec)
spec.loader.exec_module(harness)

def test_e2e_must_not_answer_offer_in_inherited_home(tmp_path):
    home = tmp_path / 'caller-home'
    home.mkdir(mode=0o700)
    data = tmp_path / 'fixture-data'
    data.mkdir(mode=0o700)
    workspace = tmp_path / 'workspace'
    workspace.mkdir()
    binary = tmp_path / 'fake-bus'
    binary.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys, time
answer = pathlib.Path(os.environ["HOME"]) / "answer"
if len(sys.argv) > 2 and sys.argv[2] == "state":
 print(json.dumps({"ok": True, "result": {"rooms": [{"name": "bus"}]}})); sys.exit(0)
print("Install? [y/N] ", end="", flush=True)
answer.write_text(sys.stdin.readline().strip())
time.sleep(30)
''')
    binary.chmod(0o755)
    env = harness.scrubbed_env({**os.environ, 'HOME': str(home)}, data)
    bus = harness.Bus(binary, tmp_path, env, workspace=workspace)
    try:
        bus.start()
        assert not (home / 'answer').exists(), (home / 'answer').read_text()
    finally:
        if bus.child and bus.child.poll() is None:
            os.killpg(bus.child.pid, signal.SIGTERM)
            bus.child.wait(timeout=5)
        if bus.master is not None:
            os.close(bus.master)
