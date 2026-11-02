#!/usr/bin/env python3
import argparse
import json
import os
from pathlib import Path
import stat
import subprocess
import tempfile
import time
import uuid

from tools.acceptance.harness_test import Bus
from tools.acceptance.event_delivery_test import Scratch

REPO = Path(__file__).resolve().parents[2]


class RealBus(Bus):
    def prepare_skills(self):
        home = Path(self.env['HOME'])
        protected = Path(self.env['TEST_BUS_PROTECTED_HOME'])
        assert home.resolve() == protected.resolve(), 'real provider acceptance requires the real HOME'
        mode = stat.S_IMODE(self.data.stat().st_mode)
        assert mode == 0o700, oct(mode)
        assert not str(self.data).startswith(str(protected / '.local/share/bus'))


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
    args = parser.parse_args()
    binary = Path(args.binary).resolve()
    expected = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=REPO, text=True).strip()
    version = subprocess.check_output([binary, '--version'], text=True)
    assert expected in version, (expected, version)
    root = Path(tempfile.mkdtemp(prefix='ber-', dir='/private/tmp'))
    data = root / 'data'
    bindir = root / 'bin'
    data.mkdir(mode=0o700)
    bindir.mkdir(mode=0o700)
    root.chmod(0o700)
    (bindir / 'bus').symlink_to(binary)
    (bindir / 'codex').symlink_to('/opt/homebrew/bin/codex')
    (bindir / 'kimi').symlink_to('/Users/dylanliu/.kimi-code/bin/kimi')
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
    report = {'head': expected, 'binary': str(binary), 'providers': {}, 'records': []}
    try:
        bus = RealBus(binary, artifacts, env, workspace=REPO)
        bus.start()
        idle_started = time.monotonic()
        idle_cpu_started = cpu_times(os.getpgid(bus.child.pid))
        time.sleep(3)
        idle_elapsed = time.monotonic() - idle_started
        report['idle_cpu_percent'] = (cpu_times(os.getpgid(bus.child.pid)) - idle_cpu_started) / idle_elapsed * 100
        assert report['idle_cpu_percent'] < 2.0, report['idle_cpu_percent']
        for provider in ('codex', 'kimi'):
            bus.cli('agent', 'add', '--room', 'bus', '--name', provider, '--provider', provider,
                    '--pwd', str(REPO), '--consent-hooks')
            bus.wait_native_ready(provider)
            screen = bus.cli('agent', 'observe', provider).get('text', '').lower()
            assert not any(prompt in screen for prompt in ('trust this folder', 'do you trust', 'would you like to run', 'do you want to proceed')), 'unexpected trust or permission dialog'
            marker = f'EVENT_{provider.upper()}_OK'
            record = send(bus, provider, f'Reply with {marker} exactly, with no other text.')
            assert record['correct'], record
            report['records'].append(record)
            report['providers'][provider] = {'first_reply': True}
        bus.cli('quit')
        bus.child.wait(timeout=15)
        bus = RealBus(binary, artifacts, env, workspace=REPO)
        bus.start()
        for provider in ('codex', 'kimi'):
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
        shutil.rmtree(root, ignore_errors=True)


if __name__ == '__main__':
    main()
