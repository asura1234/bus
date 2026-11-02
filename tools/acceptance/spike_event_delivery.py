#!/usr/bin/env python3
import argparse
import concurrent.futures
import datetime
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import time

ROOT = Path('/tmp/btc-event-delivery')
RUNS = Path('/tmp/bus-spike-event-delivery-runs')
PAYLOAD = Path('/Users/dylanliu/work/bus/temp/unsent-draft/review-reply-payload.txt').read_text()
assert len(PAYLOAD.encode()) == 823, len(PAYLOAD.encode())


def fnv(data):
    value = 0xcbf29ce484222325
    for byte in data:
        value = ((value ^ byte) * 0x100000001b3) & 0xffffffffffffffff
    return value


def log_time(line):
    try:
        return datetime.datetime.fromisoformat(line.split(' ', 1)[0].replace('Z', '+00:00')).timestamp()
    except (ValueError, IndexError):
        return None


class Scratch:
    def __init__(self, binary, name, flag):
        self.binary = str(binary)
        self.name = name
        self.flag = flag
        self.records = []
        self.run_dir = None
        self.env = os.environ.copy()
        for key in list(self.env):
            if key.startswith('BUS_') or key.startswith('HERDR_'):
                self.env.pop(key)
        self.env.update(BUS_TUI_ALLOW_TARGET_DEBUG='1', BUS_TUI_ROOT=str(ROOT), BUS_TUI_REAP_WITH=str(os.getpid()))
        if flag:
            self.env[flag] = '1'

    def call(self, args, timeout=40):
        return subprocess.run([self.binary, '--dev', 'tui', *args, '--name', self.name], env=self.env, text=True, capture_output=True, timeout=timeout)

    def ok(self, args, timeout=40):
        result = self.call(args, timeout)
        try:
            value = json.loads(result.stdout.strip().splitlines()[-1])
        except Exception:
            raise RuntimeError((args, result.returncode, result.stdout, result.stderr))
        if result.returncode or value.get('ok') is False:
            raise RuntimeError((args, result.returncode, value, result.stderr))
        return value

    def bus(self, args, timeout=40):
        result = subprocess.run([self.binary, '--dev', 'tui', 'bus', '--name', self.name, *args], env=self.env, text=True, capture_output=True, timeout=timeout)
        try:
            value = json.loads(result.stdout.strip())
        except Exception:
            raise RuntimeError((args, result.returncode, result.stdout, result.stderr))
        if result.returncode or not value.get('ok', False):
            raise RuntimeError((args, result.returncode, value, result.stderr))
        return value

    def start(self):
        self.ok(['start', '--size', '140x40', '--run-dir', str(RUNS)], 60)
        self.ok(['wait', '--text', '# bus', '--timeout', '15s'])
        work = ROOT / self.name / 'work'
        work.mkdir(parents=True, exist_ok=True)
        self.bus(['agent', 'add', '--room', 'bus', '--name', 'a1', '--provider', 'claude', '--pwd', str(work)])
        self.bus(['agent', 'add', '--room', 'bus', '--name', 'cdlg', '--provider', 'codex', '--pwd', str(work), '--consent-hooks'])
        self.wait_idle(['a1', 'cdlg'])

    def wait_idle(self, names, timeout=45):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            state = self.bus(['state'])['result']
            statuses = {a.get('name'): a.get('status') for a in state.get('agents', [])}
            if all(statuses.get(name) == 'idle' for name in names):
                return
            time.sleep(.05)
        raise RuntimeError(('idle timeout', names, statuses))

    def request(self, message_id):
        status = self.bus(['message', 'status', message_id])['result']
        return next(r for r in status['requests'] if r.get('agent_id') is not None)

    def send(self, agent, text, scenario, wait=True):
        before_call = time.monotonic()
        receipt = self.bus(['send', '--room', 'bus', '--to', agent, '--text', text])
        accepted_mono = time.monotonic()
        accepted_unix = time.time()
        message_id = str(receipt['result']['message_id'])
        prompt = f'[Bus message {message_id} from you]\n{text}'.encode()
        record = {'scenario': scenario, 'agent': agent, 'id': message_id, 'marker': text.splitlines()[0],
                  'accepted_call_ms': (accepted_mono - before_call) * 1000,
                  'accepted_mono': accepted_mono, 'accepted_unix': accepted_unix,
                  'hashes': [fnv(prompt), fnv(b'\x1b[200~' + prompt + b'\x1b[201~')],
                  'started_ms': None, 'correct': False, 'draft_exact': None}
        self.records.append(record)
        if wait:
            self.wait_reply(record)
        return record

    def wait_reply(self, record):
        deadline = time.monotonic() + 45
        while time.monotonic() < deadline:
            request = self.request(record['id'])
            if record['started_ms'] is None and request.get('start_bound'):
                record['started_ms'] = (time.monotonic() - record['accepted_mono']) * 1000
            if request.get('stage') == 'replied':
                record['correct'] = record['marker'] in json.dumps(request)
                record['status'] = request
                return record
            if request.get('stage') in ('abandoned', 'failed'):
                record['status'] = request
                return record
            time.sleep(.005)
        record['status'] = request
        return record

    def strings(self, value):
        if isinstance(value, str):
            yield value
        elif isinstance(value, dict):
            for child in value.values():
                yield from self.strings(child)
        elif isinstance(value, list):
            for child in value:
                yield from self.strings(child)

    def s1(self):
        for i in range(10):
            self.send('a1', f'S1-{self.name}-{i} empty idle', 'S1')

    def s2(self):
        for i in range(3):
            image = ROOT / self.name / 'work' / 'shot.png'
            image.write_bytes(b'fake image')
            draft = f'"{image}"\nfake:draft-bytes S2-{self.name}-{i}\n' + ''.join(f'rich line {j:02d}\n' for j in range(12))
            self.ok(['click', '--hit', 'agent:a1'])
            self.ok(['paste', draft])
            record = self.send('a1', f'S2-{self.name}-{i} preserve draft', 'S2', wait=False)
            time.sleep(2)
            initial = self.request(record['id'])
            record['initial_stage'] = initial.get('stage')
            record['initial_reason'] = initial.get('reason')
            self.ok(['type', '-END'])
            self.ok(['press', 'enter'])
            self.ok(['wait', '--text', 'fake draft bytes:', '--timeout', '20s'])
            transcript = self.bus(['agent', 'read', 'a1', '--source', 'recent', '--lines', '200'])
            replies = [s for s in self.strings(transcript) if 'fake draft bytes: ' in s]
            if not replies:
                raise RuntimeError(('draft reply missing', transcript))
            restored = json.loads(replies[-1].split('fake draft bytes: ', 1)[1].replace('\n', ''))
            if restored != draft + '-END':
                raise RuntimeError(('draft mismatch', repr(restored), repr(draft + '-END')))
            record['draft_exact'] = True
            self.wait_reply(record)
            record['correct'] = record['correct'] and record['status'].get('stage') == 'replied'

    def s3(self):
        lead = self.send('a1', f'fake:work 15 S3-lead-{self.name}', 'S3-lead', wait=False)
        self.ok(['wait', '--state', 'agents.*.status == working', '--timeout', '20s'])
        steered = [self.send('a1', f'S3-{self.name}-{i}\n{PAYLOAD}', 'S3', wait=False) for i in range(3)]
        self.wait_reply(lead)
        for record in steered:
            self.wait_reply(record)

    def s4(self):
        for i in range(2):
            lead = self.send('cdlg', 'fake:dialog-settle redraw', 'S4-lead', wait=False)
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                dialog = self.bus(['agent', 'dialog', 'cdlg'])['result']
                if dialog.get('dialog'):
                    break
                time.sleep(.05)
            else:
                raise RuntimeError('dialog did not appear')
            blocked = self.send('cdlg', f'S4-{self.name}-{i} must not enter', 'S4', wait=False)
            time.sleep(1)
            status = self.bus(['message', 'status', blocked['id']])['result']
            screen = self.bus(['agent', 'read', 'cdlg', '--source', 'visible'])
            blocked['dialog_safe'] = not any(r.get('stage') == 'replied' for r in status['requests']) and 'You approved' not in json.dumps(screen)
            blocked['correct'] = blocked['dialog_safe']
            dialog = self.bus(['agent', 'dialog', 'cdlg'])['result']
            self.bus(['agent', 'choose', 'cdlg', '--option', '1', '--fingerprint', dialog['fingerprint']])
            self.wait_reply(lead)
            self.wait_reply(blocked)
            blocked['correct'] = blocked['dialog_safe'] and blocked.get('status', {}).get('stage') == 'replied'

    def s5(self):
        for i in range(30):
            self.send('a1', f'S5-serial-{self.name}-{i}', 'S5-serial')
        work = ROOT / self.name / 'work'
        for name in ['a2', 'a3', 'a4', 'a5']:
            self.bus(['agent', 'add', '--room', 'bus', '--name', name, '--provider', 'claude', '--pwd', str(work)])
        self.wait_idle(['a2', 'a3', 'a4', 'a5'])
        with concurrent.futures.ThreadPoolExecutor(max_workers=10) as pool:
            jobs = [pool.submit(self.send, f'a{i % 5 + 1}', f'S5-parallel-{self.name}-{i}', 'S5-parallel') for i in range(10)]
            for job in jobs:
                job.result()

    def stop(self):
        stopped = self.ok(['stop'], 30)
        self.run_dir = Path(stopped['run_dir'])

    def attach_metrics(self):
        events = {}
        for log in self.run_dir.joinpath('logs').rglob('*server.log*'):
            if not log.is_file() or log.suffix == '.gz':
                continue
            for line in log.read_text(errors='replace').splitlines():
                if 'bus.terminal.submission.' not in line:
                    continue
                event = re.search(r'bus\.terminal\.submission\.(paste_flushed|enter_flushed)', line)
                delivery = re.search(r'delivery[= ](\d+)', line)
                elapsed = re.search(r'elapsed_ms[= ](\d+)', line)
                if not event or not delivery or not elapsed:
                    continue
                key = (delivery.group(1), event.group(1))
                events.setdefault(key, []).append((int(elapsed.group(1)), log_time(line), line))
        for record in self.records:
            for metric, event in [('paste_ms', 'paste_flushed'), ('enter_ms', 'enter_flushed')]:
                matches = []
                for digest in record['hashes']:
                    matches.extend(events.get((str(digest), event), []))
                successful = [(ms, at) for ms, at, line in matches if 'flushed=true' in line or 'flushed = true' in line]
                record[metric] = min(ms for ms, _ in successful) if successful else None
                times = [at for _, at in successful if at is not None]
                record[metric.replace('_ms', '_accepted_ms')] = (min(times) - record['accepted_unix']) * 1000 if times else None
        return {'records': self.records, 'run_dir': str(self.run_dir)}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', required=True)
    parser.add_argument('--output', required=True)
    parser.add_argument('--variants', default='baseline,coordinator,paste,detection,room,full')
    args = parser.parse_args()
    RUNS.mkdir(parents=True, exist_ok=True)
    flags = {'baseline': None, 'coordinator': 'BUS_SPIKE_COORDINATOR_WAKE', 'paste': 'BUS_SPIKE_ZERO_PASTE',
             'detection': 'BUS_SPIKE_DETECTION_WAKE', 'room': 'BUS_SPIKE_ROOM_PUSH', 'full': 'BUS_SPIKE_EVENT_DELIVERY'}
    output = {}
    for variant in args.variants.split(','):
        name = variant + str(os.getpid())
        scratch = Scratch(args.binary, name, flags[variant])
        try:
            scratch.start()
            scenarios = [scratch.s1] if variant not in ('baseline', 'full') else [scratch.s1, scratch.s2, scratch.s3, scratch.s4, scratch.s5]
            for scenario in scenarios:
                started = time.monotonic()
                scenario()
                print(name, scenario.__name__, round(time.monotonic() - started, 1), flush=True)
        finally:
            scratch.stop()
            output[variant] = scratch.attach_metrics()
            Path(args.output).write_text(json.dumps(output, indent=2))
    shutil.rmtree(ROOT, ignore_errors=True)


if __name__ == '__main__':
    main()
