import json
import os
from pathlib import Path
import signal
import sqlite3
import subprocess
import tempfile
import time
import unittest


BINARY = Path(os.environ["FEISHU_BRIDGE_BINARY"]).resolve()


class SinkProcessTimeout(unittest.TestCase):
    def test_bounded_listener_exits_when_sink_descendant_retains_stdin(self):
        with tempfile.TemporaryDirectory(prefix="feishu-sink-timeout-") as folder:
            root = Path(folder)
            app = "cli_pipe_timeout_probe"
            state = root / "state"
            config = root / "config.toml"
            events = root / "events.ndjson"
            sink = root / "sink.py"
            lark = root / "lark.py"
            descendant_pid = root / "descendant.pid"
            env = {key: value for key, value in os.environ.items()
                   if not key.startswith(("BUS_", "HERDR_"))}
            env.update(HOME=str(root), XDG_CONFIG_HOME=str(root / "config"))
            lark.write_text("""#!/bin/sh
case "$1" in
auth) printf '%s\\n' '{"appId":"cli_pipe_timeout_probe","identities":{"bot":{"status":"ready","available":true}}}' ;;
im) printf '%s\\n' '{"ok":true,"data":{"chat_mode":"p2p"}}' ;;
api) printf '%s\\n' '{"ok":true,"data":{"items":[],"has_more":false}}' ;;
event) printf '%s\\n' '[event] ready event_key=im.message.receive_v1' >&2; /bin/sleep 30 ;;
*) exit 2 ;;
esac
""")
            lark.chmod(0o700)

            def write_config(executable):
                config.write_text(
                    f'app_id = "{app}"\n'
                    f'lark_cli = {json.dumps(str(lark))}\n'
                    f'state_dir = {json.dumps(str(state))}\n'
                    'allowed_sender_open_ids = ["ou_allowed"]\n'
                    'allowed_chat_ids = ["oc_allowed"]\n'
                    f'sink_command = [{json.dumps(str(executable))}]\n'
                    'command_timeout_seconds = 3\n'
                    'sink_timeout_seconds = 1\n'
                    'retry_seconds = 1\n'
                    'reconcile_seconds = 60\n'
                )

            def run(*args, **kwargs):
                return subprocess.run(
                    [str(BINARY), '--config', str(config), *args], env=env,
                    capture_output=True, text=True, timeout=4, **kwargs)

            # Seed a real durable pending message through the public import route.
            write_config('/usr/bin/false')
            events.write_text(json.dumps({
                'type': 'im.message.receive_v1', 'message_id': 'om_pipe',
                'chat_id': 'oc_allowed', 'chat_type': 'p2p',
                'sender_id': 'ou_allowed', 'sender_type': 'user',
                'message_type': 'text', 'content': 'x' * (1024 * 1024),
                'create_time': '1000'
            }) + '\n')
            seeded = run('listen', '--input', str(events))
            self.assertEqual(seeded.returncode, 1, seeded.stderr)
            self.assertEqual(json.loads(run('status').stdout)['pending'], 1)

            # A sink may fail after launching a helper. The helper inherits stdin
            # but has its own session, so it outlives process-group cleanup.
            sink.write_text("""#!/usr/bin/env python3
import pathlib,subprocess,sys
child=subprocess.Popen(['/bin/sleep','30'],start_new_session=True,stdin=sys.stdin)
pathlib.Path(__file__).with_name('descendant.pid').write_text(str(child.pid))
sys.exit(7)
""")
            sink.chmod(0o700)
            write_config(sink)
            child = subprocess.Popen(
                [str(BINARY), '--config', str(config), 'listen', '--run-for', '3'],
                env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                text=True, start_new_session=True)
            started = time.monotonic()
            expired = False
            try:
                try:
                    stdout, stderr = child.communicate(timeout=7)
                except subprocess.TimeoutExpired:
                    expired = True
                    # Unblock only the test's escaped helper before collecting
                    # the bridge's result; never touch another process.
                    if descendant_pid.exists():
                        os.kill(int(descendant_pid.read_text()), signal.SIGKILL)
                    stdout, stderr = child.communicate(timeout=3)
                with sqlite3.connect(state / 'spool.sqlite3') as conn:
                    rows = conn.execute(
                        'SELECT id,delivered FROM messages ORDER BY seq').fetchall()
                print(json.dumps({
                    'test': self._testMethodName,
                    'binary': str(BINARY), 'timed_out_after_seven_seconds': expired,
                    'elapsed_seconds': round(time.monotonic() - started, 3),
                    'bridge_exit': child.returncode,
                    'sink_helper_started': descendant_pid.exists(),
                    'stored_messages': rows,
                    'bridge_stdout': stdout, 'bridge_stderr': stderr,
                }), flush=True)
                self.assertTrue(descendant_pid.exists(), 'sink was not reached')
                self.assertFalse(
                    expired,
                    '--run-for 3 and sink_timeout_seconds=1 left the bridge '
                    'stuck for more than seven seconds until the escaped stdin '
                    'holder was killed externally')
                self.assertEqual(child.returncode, 0, stderr)
                self.assertEqual(rows, [('om_pipe', 0)])
            finally:
                if descendant_pid.exists():
                    try:
                        os.kill(int(descendant_pid.read_text()), signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                if child.poll() is None:
                    os.killpg(child.pid, signal.SIGKILL)
                    child.communicate(timeout=3)


if __name__ == '__main__':
    unittest.main()
