#!/usr/bin/env python3
import argparse
import concurrent.futures
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import time

ROOT = Path('/tmp/btc-kimispikec')
RUNS = Path('/tmp/bus-spike-kimispikec/runs')
PAYLOAD = Path('/Users/dylanliu/work/bus/temp/unsent-draft/review-reply-payload.txt').read_text()
assert len(PAYLOAD.encode()) == 823, len(PAYLOAD.encode())


def fnv(data):
    value = 0xcbf29ce484222325
    for byte in data:
        value = ((value ^ byte) * 0x100000001b3) & 0xffffffffffffffff
    return value


class Scratch:
    def __init__(self, binary, name, fast):
        self.binary = str(binary)
        self.name = name
        self.fast = fast
        self.records = []
        self.run_dir = None
        self.env = os.environ.copy()
        for key in list(self.env):
            if key.startswith('BUS_') or key.startswith('HERDR_'):
                self.env.pop(key)
        self.env.update(BUS_TUI_ALLOW_TARGET_DEBUG='1', BUS_TUI_ROOT=str(ROOT), BUS_TUI_REAP_WITH=str(os.getpid()))
        if fast:
            self.env['BUS_SPIKE_SKIP_CONFIRM'] = '1'

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
        work = ROOT / self.name / self.name / 'work'
        work.mkdir(parents=True, exist_ok=True)
        for name in ['a1', 'a2', 'a3', 'a4', 'a5']:
            self.bus(['agent', 'add', '--room', 'bus', '--name', name, '--provider', 'claude', '--pwd', str(work)])
        self.bus(['agent', 'add', '--room', 'bus', '--name', 'cdlg', '--provider', 'codex', '--pwd', str(work), '--consent-hooks'])
        self.wait_idle(['a1', 'a2', 'a3', 'a4', 'a5', 'cdlg'])

    def wait_idle(self, names, timeout=45):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            state = self.bus(['state'])['result']
            agents = state.get('agents', [])
            statuses = {a.get('name'): a.get('status') for a in agents}
            if all(statuses.get(name) == 'idle' for name in names):
                return
            time.sleep(.1)
        raise RuntimeError(('idle timeout', names, statuses))

    def send(self, agent, text, scenario=None, wait=True):
        before_call = time.monotonic()
        receipt = self.bus(['send', '--room', 'bus', '--to', agent, '--text', text])
        accepted_mono = time.monotonic()
        accepted_unix = time.time()
        message_id = str(receipt['result']['message_id'])
        prompt = f'[Bus message {message_id} from you]\n{text}'.encode()
        record = {'scenario': scenario or text.split('-', 1)[0], 'agent': agent, 'id': message_id, 'marker': text.splitlines()[0], 'accepted_call_ms': (accepted_mono - before_call) * 1000,
                  'accepted_mono': accepted_mono, 'accepted_unix': accepted_unix,
                  'hashes': [fnv(prompt), fnv(b'\x1b[200~' + prompt + b'\x1b[201~')], 'started_ms': None, 'correct': False}
        deadline = accepted_mono + 45
        while time.monotonic() < deadline:
            status = self.bus(['message', 'status', message_id])['result']
            requests = status['requests']
            request = next(r for r in requests if r.get('agent_id') is not None)
            if record['started_ms'] is None and request.get('start_bound'):
                record['started_ms'] = (time.monotonic() - record['accepted_mono']) * 1000
            if wait and request.get('stage') == 'replied':
                record['correct'] = record['marker'] in json.dumps(request)
                record['status'] = request
                self.records.append(record)
                return record
            if not wait:
                self.records.append(record)
                return record
            if request.get('stage') in ('abandoned', 'failed'):
                record['status'] = request
                self.records.append(record)
                return record
            time.sleep(.005)
        record['status'] = request
        self.records.append(record)
        return record

    def wait_reply(self, record):
        deadline = time.monotonic() + 45
        while time.monotonic() < deadline:
            status = self.bus(['message', 'status', record['id']])['result']
            request = next(r for r in status['requests'] if r.get('agent_id') is not None)
            if record['started_ms'] is None and request.get('start_bound'):
                record['started_ms'] = (time.monotonic() - record['accepted_mono']) * 1000
            if request.get('stage') == 'replied':
                record['correct'] = record['marker'] in json.dumps(request)
                record['status'] = request
                return record
            if request.get('stage') in ('abandoned', 'failed'):
                record['status'] = request
                return record
            time.sleep(.02)
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
            self.send('a1', f'S1-{self.name}-{i} empty idle')

    def s2(self):
        for i in range(1):
            draft = f'  fake:draft-bytes S2-{self.name}-{i}\t中 文\nsecond  line\n\n  tail\t  \n'
            self.ok(['click', '--hit', 'agent:a1'])
            self.ok(['wait', '--text', '? for shortcuts · fake claude', '--timeout', '10s'])
            time.sleep(.3)
            self.ok(['type', draft])
            time.sleep(3.2)
            delivered = self.send('a1', f'S2-{self.name}-{i} preserve draft')
            if not delivered['correct']:
                raise RuntimeError(('S2 delivery failed', delivered))
            self.ok(['type', '-END'])
            self.ok(['press', 'enter'])
            self.ok(['wait', '--text', 'fake draft bytes', '--timeout', '20s'])
            time.sleep(.5)
            native = self.bus(['agent', 'read', 'a1', '--source', 'visible'])
            replies = [s[s.index('fake draft bytes: '):] for s in self.strings(native) if 'fake draft bytes: ' in s]
            restored = None
            for reply in replies:
                try:
                    restored = json.JSONDecoder().raw_decode(reply.split(': ', 1)[1].replace('\n', '').replace('\r', ''))[0]
                except (ValueError, json.JSONDecodeError):
                    pass
            if restored is None:
                raise RuntimeError(('draft reply unavailable', native))
            if restored != draft + '-END':
                raise RuntimeError(('draft mismatch', repr(restored), repr(draft + '-END')))
            self.records[-1]['draft_exact'] = True

    def s3(self):
        lead = self.send('a1', f'fake:work 30 S3-lead-{self.name}', wait=False)
        deadline = time.monotonic() + 20
        while not lead['started_ms'] and time.monotonic() < deadline:
            status = self.bus(['message', 'status', lead['id']])['result']
            if status['requests'][0].get('start_bound'):
                lead['started_ms'] = (time.monotonic() - lead['accepted_mono']) * 1000
            time.sleep(.005)
        if not lead['started_ms']:
            raise RuntimeError(('S3 lead did not start', lead))
        steered = [self.send('a1', f'S3-{self.name}-{i}\n{PAYLOAD}', wait=False) for i in range(3)]
        pending = list(steered)
        while pending and time.monotonic() < deadline:
            for record in list(pending):
                status = self.bus(['message', 'status', record['id']])['result']
                if status['requests'][0].get('start_bound'):
                    record['started_ms'] = (time.monotonic() - record['accepted_mono']) * 1000
                    pending.remove(record)
            time.sleep(.005)
        self.wait_reply(lead)
        for record in steered:
            self.wait_reply(record)

    def s4(self):
        for i in range(2):
            lead = self.send('cdlg', 'go fake:dialog 20', wait=False)
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                dialog = self.bus(['agent', 'dialog', 'cdlg'])['result']
                if dialog.get('dialog'):
                    break
                time.sleep(.05)
            else:
                raise RuntimeError('dialog did not appear')
            blocked = self.send('cdlg', f'S4-{self.name}-{i} must not enter', wait=False)
            time.sleep(1)
            status = self.bus(['message', 'status', blocked['id']])['result']
            screen = self.bus(['agent', 'read', 'cdlg', '--source', 'visible'])
            blocked['correct'] = not any(r.get('stage') == 'replied' for r in status['requests']) and 'Do you want to proceed?' in json.dumps(screen)
            dialog = self.bus(['agent', 'dialog', 'cdlg'])['result']
            self.bus(['agent', 'choose', 'cdlg', '--option', '1', '--fingerprint', dialog['fingerprint']])
            self.wait_reply(lead)
            self.wait_reply(blocked)

    def s5(self):
        for i in range(30):
            self.send('a1', f'S5-serial-{self.name}-{i}')
        with concurrent.futures.ThreadPoolExecutor(max_workers=10) as pool:
            jobs = [pool.submit(self.send, f'a{i % 5 + 1}', f'S5-parallel-{self.name}-{i}') for i in range(10)]
            for job in jobs:
                job.result()

    def stop(self):
        stopped = self.ok(['stop'], 30)
        self.run_dir = Path(stopped['run_dir'])

    def attach_metrics(self):
        events = {}
        for log in self.run_dir.joinpath('logs').rglob('herdr-server.log*'):
            if not log.is_file() or log.suffix == '.gz':
                continue
            for line in log.read_text(errors='replace').splitlines():
                if 'bus.terminal.submission.' not in line:
                    continue
                event = re.search(r'bus\.terminal\.submission\.(paste_flushed|enter_flushed|fast_path|summary)', line)
                delivery = re.search(r'delivery[= ](\d+)', line)
                elapsed = re.search(r'elapsed_ms[= ](\d+)', line)
                if not event or not delivery or not elapsed:
                    continue
                key = (delivery.group(1), event.group(1))
                try:
                    wall = __import__('datetime').datetime.fromisoformat(line.split(None, 1)[0].replace('Z', '+00:00')).timestamp()
                except (ValueError, TypeError):
                    wall = None
                events.setdefault(key, []).append((int(elapsed.group(1)), wall, line))
        for record in self.records:
            for metric, event in [('paste', 'paste_flushed'), ('enter', 'enter_flushed')]:
                matches = []
                for digest in record['hashes']:
                    matches.extend(events.get((str(digest), event), []))
                successful = [(native, wall) for native, wall, line in matches if 'flushed=true' in line or 'flushed = true' in line]
                record[metric + '_native_ms'] = min((native for native, wall in successful), default=None)
                record[metric + '_ms'] = min(((wall - record['accepted_unix']) * 1000 for native, wall in successful if wall is not None), default=None)
            record['fast'] = any((str(digest), 'fast_path') in events for digest in record['hashes'])
        return {'records': self.records, 'run_dir': str(self.run_dir)}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', required=True)
    parser.add_argument('--output', required=True)
    args = parser.parse_args()
    RUNS.mkdir(parents=True, exist_ok=True)
    output = {}
    for fast in (False, True):
        name = ('after' if fast else 'before') + str(os.getpid())
        scratch = Scratch(args.binary, name, fast)
        try:
            scratch.start()
            for scenario in (scratch.s1, scratch.s2, scratch.s3, scratch.s4, scratch.s5):
                started = time.monotonic()
                scenario()
                print(name, scenario.__name__, round(time.monotonic() - started, 1), flush=True)
        finally:
            scratch.stop()
        output['after' if fast else 'before'] = scratch.attach_metrics()
        Path(args.output).write_text(json.dumps(output, indent=2))
    shutil.rmtree(ROOT, ignore_errors=True)


if __name__ == '__main__':
    main()
