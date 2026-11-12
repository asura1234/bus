"""Scratch-only overload→success acceptance; no credentials or real providers."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import uuid

from tools.acceptance.harness_test import Bus


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--expected-head', required=True)
    parser.add_argument('--evidence-dir', type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    artifacts = args.evidence_dir.resolve()
    artifacts.mkdir(parents=True, exist_ok=True, mode=0o700)
    root = Path(tempfile.mkdtemp(prefix='bpf-', dir='/private/tmp'))
    root.chmod(0o700)
    for name in ('home', 'data', 'work', 'bin'):
        (root / name).mkdir(mode=0o700)
    for name in ('claude', 'codex', 'cursor-agent', 'grok', 'open', 'osascript', 'afplay', 'terminal-notifier'):
        (root / 'bin' / name).symlink_to(binary)
    search = [root / 'bin', Path('/usr/bin'), Path('/bin')]
    assert all(not (p / 'feishu-bridge').exists() for p in search)
    env = dict(HOME=str(root / 'home'), USERPROFILE=str(root / 'home'), SHELL='/bin/zsh',
               PATH=':'.join(map(str, search)), TERM='xterm-256color', LANG='en_US.UTF-8',
               BUS_DATA_DIR=str(root / 'data'), BUS_TUI_FAKE_AGENT='1',
               BUS_TUI_OWNER=uuid.uuid4().hex, BUS_TUI_EFFECTS=str(root / 'effects.ndjson'),
               TEST_BUS_PROTECTED_HOME=os.environ['HOME'])
    subprocess.run(['/usr/bin/git', 'init', '--quiet', str(root / 'work')], env=env, check=True)
    identity = json.loads(subprocess.check_output([str(binary), '--identity'], env=env))
    assert identity['sha'] == args.expected_head and 'claude' in identity['providers'], identity
    assert b'fake:overload-once' in binary.read_bytes(), 'fault injection feature missing'
    paths = json.loads(subprocess.check_output([str(binary), '--paths'], env=env))
    assert Path(paths['data']).resolve() == root / 'data', paths
    bus = Bus(binary, artifacts, env, workspace=root / 'work')
    report = dict(expected_head=args.expected_head, binary=str(binary), identity=identity,
                  binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
                  scratch_root=str(root), provenance='synthetic provider, real Bus delivery path', samples=[])
    trace = artifacts / 'commands.jsonl'

    def cli(*argv):
        result = bus.cli(*map(str, argv))
        with trace.open('a') as f:
            f.write(json.dumps(dict(at=time.time(), argv=list(map(str, argv)), result=result)) + '\n')
        return result

    def until(read, accept, timeout=40):
        deadline = time.monotonic() + timeout
        while True:
            value = read()
            if accept(value):
                return value
            if time.monotonic() >= deadline:
                raise AssertionError(value)
            time.sleep(0.1)

    try:
        bus.start()
        logs = Path(paths['logs'])
        starts = []
        for role in ('client', 'server'):
            path = logs / 'perf' / (role + '.jsonl')
            rows = until(lambda: [json.loads(l) for l in path.read_text().splitlines()] if path.exists() else [],
                         lambda rows: any(r.get('kind') == 'run_start' for r in rows))
            start = next(r for r in rows if r.get('kind') == 'run_start')
            assert start['build_sha'] == args.expected_head and start['dev_tools'], start
            starts.append(start)
        report['run_start'] = starts
        room = cli('room', 'create', 'provider-flake')['room_id']
        agent = cli('agent', 'add', '--room', room, '--name', 'flake-worker', '--provider', 'claude',
                    '--pwd', root / 'work')['agent_id']
        managed = until(lambda: next(a for a in cli('state')['agents'] if a['name'] == 'flake-worker'),
                        lambda a: a['runtime_identity'].get('pane_id'))
        report['native_ready'] = bus.wait_native_ready(managed['runtime_identity']['pane_id'])
        sent = cli('send', '--room', room, '--to', agent,
                   '--text', 'fake:overload-once fake:raw-reply PROVIDER_FLAKE_RECOVERED')
        message, request = sent['message_id'], sent['request_ids'][0]
        client_log = logs / 'herdr-client.log'

        def status():
            value = cli('message', 'status', message)
            report['samples'].append(value)
            return value

        held = until(status, lambda s: not s['complete']
                     and s['requests'][0]['status'] == 'working'
                     and client_log.exists() and 'bus.provider.hold' in client_log.read_text())
        assert held['requests'][0]['request_id'] == request
        assert held['requests'][0]['stage'] != 'abandoned'
        report['held'] = held
        done = until(status, lambda s: s['complete'])
        row, = done['requests']
        assert row['request_id'] == request and row['stage'] == 'replied', done
        assert row['reply']['text'] == 'PROVIDER_FLAKE_RECOVERED' and not row['uncertain_outcome'], done
        history = cli('history', '--room', room, '--limit', 20)
        assert sum(m['prompt']['id'] == message for m in history['messages']) == 1
        text = client_log.read_text()
        assert text.count('event="bus.provider.hold"') == 1, text[-6000:]
        assert text.count('event="bus.provider.retry"') == 1, text[-6000:]
        report.update(result='PASS', message_id=message, request_id=request, final=done, history=history)
    except Exception as error:
        report.update(result='FAIL', error=f'{type(error).__name__}: {error}')
        raise
    finally:
        report['cleanup'] = bus.stop()
        logs = Path(paths['logs'])
        for name in ('herdr-client.log', 'herdr-server.log'):
            path = logs / name
            if path.is_file():
                shutil.copy2(path, artifacts / name)
        assert not report['cleanup']['leftover'], report['cleanup']
        shutil.rmtree(root)
        report['scratch_deleted'] = not root.exists()
        (artifacts / 'report.json').write_text(json.dumps(report, indent=2))
        print(json.dumps(dict(result=report.get('result'), report=str(artifacts / 'report.json'))))


if __name__ == '__main__':
    main()
