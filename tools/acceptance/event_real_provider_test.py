#!/usr/bin/env python3
import argparse
import json
import os
from pathlib import Path
import re
import stat
import signal
import socket
import subprocess
import tempfile
import time
import uuid
import sys

REPO = Path(__file__).resolve().parents[2]
if str(REPO) not in sys.path:
    sys.path.insert(0, str(REPO))

from tools.acceptance.harness_test import Bus
from tools.acceptance.event_delivery_test import Scratch


def bus_argv(binary):
    return [str(binary)]


class RealBus(Bus):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.env.pop('TEST_BUS_PROTECTED_HOME', None)
        self.env.pop('BUS_TUI_OWNER', None)
        self.env['REAL_BUS_TEST_HOME'] = self.env['HOME']

    def prepare_skills(self):
        home = Path(self.env['HOME'])
        protected = Path(self.env['REAL_BUS_TEST_HOME'])
        assert home.resolve() == protected.resolve(), 'real provider acceptance requires the real HOME'
        mode = stat.S_IMODE(self.data.stat().st_mode)
        assert mode == 0o700, oct(mode)
        assert not str(self.data).startswith(str(protected / '.local/share/bus'))

    def wait_native_ready(self, name, timeout=30):
        try:
            return super().wait_native_ready(name, timeout)
        except AssertionError as error:
            observation = self.cli('agent', 'observe', name)
            raise AssertionError(f'{error}; screen={observation}') from error

    def stop(self):
        report = {}
        try:
            self.cli('quit', timeout=15)
        except Exception as error:
            report['quit_error'] = str(error)
        if self.child is not None:
            try:
                self.child.wait(timeout=15)
            except subprocess.TimeoutExpired:
                os.killpg(self.child.pid, signal.SIGTERM)
                self.child.wait(timeout=5)
        stopped = subprocess.run([str(self.binary), 'stop'], env=self.env, capture_output=True,
                                 text=True, timeout=20, check=False)
        report['stop'] = stopped.stdout.strip() if stopped.returncode == 0 else stopped.stderr.strip()
        if self.master is not None:
            os.close(self.master)
            self.master = None
        return report


def server_pid(data):
    log = data / 'herdr-config/sessions/bus/herdr-server.log'
    for line in reversed(log.read_text().splitlines()):
        if 'event="app.startup"' in line and 'subsystem="server"' in line:
            match = re.search(r'\bpid=(\d+)', line)
            if match:
                return int(match.group(1))
    raise AssertionError('server startup PID not found')


def server_socket_open(data):
    path = data / 'herdr-config/sessions/bus/herdr.sock'
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
            stream.settimeout(.2)
            stream.connect(str(path))
        return True
    except OSError:
        return False


def pid_running(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


def cpu_times(pgid):
    result = subprocess.run(['ps', '-g', str(pgid), '-o', 'time='], text=True, capture_output=True, check=True)
    total = 0.0
    for value in result.stdout.split():
        parts = [float(part) for part in value.split(':')]
        seconds = 0.0
        for part in parts:
            seconds = seconds * 60 + part
        total += seconds
    return total


def request(bus, message_id):
    status = bus.cli('message', 'status', message_id)
    return next(row for row in status['requests'] if row.get('agent_id') is not None)


def send(bus, agent, text):
    before = time.monotonic()
    receipt = bus.cli('send', '--room', 'bus', '--to', agent, '--text', text)
    accepted = time.monotonic()
    marker = next(word for word in text.split() if word.startswith('EVENT_'))
    record = {'scenario': 'real', 'agent': agent, 'id': str(receipt['message_id']), 'marker': marker,
              'accepted_call_ms': (accepted - before) * 1000, 'accepted_mono': accepted,
              'accepted_unix': time.time(), 'started_ms': None, 'correct': False, 'draft_exact': None}
    deadline = accepted + 120
    while time.monotonic() < deadline:
        row = request(bus, record['id'])
        if row.get('start_bound') and record['started_ms'] is None:
            record['started_ms'] = (time.monotonic() - accepted) * 1000
        if row.get('stage') == 'replied':
            record['status'] = row
            record['correct'] = record['marker'] in json.dumps(row)
            return record
        if row.get('stage') in ('failed', 'abandoned'):
            raise AssertionError(row)
        time.sleep(.05)
    raise AssertionError(('real provider reply timeout', row))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', required=True)
    parser.add_argument('--output', required=True)
    parser.add_argument('--project', default=str(REPO))
    parser.add_argument('--codex-project')
    parser.add_argument('--kimi-project')
    parser.add_argument('--claude-project')
    parser.add_argument('--providers', nargs='+', choices=('codex', 'kimi', 'claude'), default=('codex', 'kimi'))
    parser.add_argument('--scratch-root')
    args = parser.parse_args()
    binary = Path(args.binary).resolve()
    projects = {provider: Path(getattr(args, f'{provider}_project') or args.project).resolve()
                for provider in ('codex', 'kimi', 'claude')}
    expected = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=REPO, text=True).strip()
    version = subprocess.check_output([binary, '--version'], text=True)
    assert expected in version, (expected, version)
    root = Path(args.scratch_root) if args.scratch_root else Path(tempfile.mkdtemp(prefix='ber-', dir='/private/tmp'))
    data = root / 'data'
    bindir = root / 'bin'
    data.mkdir(parents=True, mode=0o700, exist_ok=True)
    bindir.mkdir(mode=0o700, exist_ok=True)
    root.chmod(0o700)
    data.chmod(0o700)
    bindir.chmod(0o700)
    executables = {'bus': binary, 'codex': Path('/opt/homebrew/bin/codex'), 'node': Path('/opt/homebrew/bin/node'),
                   'kimi': Path('/Users/dylanliu/.kimi-code/bin/kimi'), 'claude': Path('/Users/dylanliu/.local/bin/claude')}
    for name, target in executables.items():
        destination = bindir / name
        if destination.is_symlink():
            destination.unlink()
        destination.symlink_to(target)
    home = Path(os.environ['HOME'])
    env = {
        'HOME': str(home), 'TEST_BUS_PROTECTED_HOME': str(home), 'USER': os.environ['USER'],
        'LOGNAME': os.environ.get('LOGNAME', os.environ['USER']), 'SHELL': '/bin/zsh',
        'TERM': 'xterm-256color', 'PATH': f'{bindir}:/usr/bin:/bin',
        'BUS_DATA_DIR': str(data), 'BUS_TUI_OWNER': uuid.uuid4().hex,
    }
    artifacts = Path(args.output)
    artifacts.mkdir(parents=True, mode=0o700, exist_ok=True)
    (artifacts / 'logs').symlink_to(data, target_is_directory=True)
    report = {'head': expected, 'binary': str(binary),
              'projects': {name: str(path) for name, path in projects.items()}, 'providers': {}, 'records': []}
    try:
        bus = RealBus(binary, artifacts, env, workspace=REPO)
        bus.start()
        time.sleep(2)
        idle_started = time.monotonic()
        idle_cpu_started = cpu_times(os.getpgid(bus.child.pid))
        time.sleep(3)
        idle_elapsed = time.monotonic() - idle_started
        report['idle_cpu_percent'] = (cpu_times(os.getpgid(bus.child.pid)) - idle_cpu_started) / idle_elapsed * 100
        assert report['idle_cpu_percent'] < 2.0, report['idle_cpu_percent']
        existing = {agent['name'] for agent in bus.cli('state').get('agents', [])}
        report['resumed_agents'] = sorted(existing.intersection(args.providers))
        for provider in args.providers:
            if provider not in existing:
                bus.cli('agent', 'add', '--room', 'bus', '--name', provider, '--provider', provider,
                        '--pwd', str(projects[provider]), '--consent-hooks')
            bus.wait_native_ready(provider)
            screen = bus.cli('agent', 'observe', provider).get('text', '').lower()
            assert not any(prompt in screen for prompt in ('trust this folder', 'do you trust', 'would you like to run', 'do you want to proceed')), 'unexpected trust or permission dialog'
            marker = f'EVENT_{provider.upper()}_OK'
            record = send(bus, provider, f'Reply with {marker} exactly, with no other text.')
            assert record['correct'], record
            report['records'].append(record)
            report['providers'][provider] = {'first_reply': True}
        old_server_pid = server_pid(data)
        bus.cli('quit')
        bus.child.wait(timeout=15)
        stopped = subprocess.run([str(binary), 'stop'], env=env, capture_output=True, text=True,
                                 timeout=20, check=True)
        report['cold_restart'] = {'stop': json.loads(stopped.stdout), 'old_server_pid': old_server_pid,
                                  'old_server_running': pid_running(old_server_pid),
                                  'old_socket_open': server_socket_open(data)}
        assert not report['cold_restart']['old_server_running'], report['cold_restart']
        assert not report['cold_restart']['old_socket_open'], report['cold_restart']
        bus = RealBus(binary, artifacts, env, workspace=REPO)
        bus.start()
        report['cold_restart']['new_server_pid'] = server_pid(data)
        report['cold_restart']['new_socket_open'] = server_socket_open(data)
        assert report['cold_restart']['new_server_pid'] != old_server_pid, report['cold_restart']
        assert report['cold_restart']['new_socket_open'], report['cold_restart']
        for provider in args.providers:
            bus.wait_native_ready(provider)
            marker = f'EVENT_{provider.upper()}_RESTART_OK'
            record = send(bus, provider, f'Reply with {marker} exactly, with no other text.')
            assert record['correct'], record
            report['records'].append(record)
            report['providers'][provider]['restart_reply'] = True
        report['stop'] = bus.stop()
        metrics = Scratch(str(binary), 'real')
        metrics.records = report['records']
        metrics.run_dir = artifacts
        report['metrics'] = metrics.attach_metrics()['records']
        (artifacts / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps({'idle_cpu_percent': report['idle_cpu_percent'], 'providers': report['providers'],
                          'records': [{k: r.get(k) for k in ('agent', 'correct', 'enter_accepted_ms', 'started_log_ms')} for r in report['metrics']]}, indent=2))
    finally:
        subprocess.run([binary, 'stop'], env=env, capture_output=True, timeout=20)
        if (artifacts / 'logs').is_symlink():
            (artifacts / 'logs').unlink()
        import shutil
        if (not args.scratch_root and report['providers']
                and all(provider.get('restart_reply') for provider in report['providers'].values())):
            shutil.rmtree(root, ignore_errors=True)
        else:
            print('retained scratch root:', root)


if __name__ == '__main__':
    main()
