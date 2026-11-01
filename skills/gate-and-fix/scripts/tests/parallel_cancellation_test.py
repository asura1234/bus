"""A nested full gate must stop every parallel worker before profile cleanup."""
import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

import pytest

SCRIPTS = Path(__file__).resolve().parents[1]


def running(pid):
    result = subprocess.run(['ps', '-p', str(pid), '-o', 'stat='], capture_output=True, text=True)
    return result.returncode == 0 and any(not state.startswith('Z') for state in result.stdout.split())


@pytest.mark.skipif(os.name != 'posix', reason='POSIX process-group contract')
@pytest.mark.parametrize('signum', [signal.SIGTERM, signal.SIGINT])
def test_nested_parallel_workers_stop_before_profiles_are_removed(tmp_path, signum):
    ready = tmp_path / 'ready'
    ready.mkdir()
    child = (
        'import json,os,signal,sys,time;from pathlib import Path;'
        'signal.signal(signal.SIGTERM,signal.SIG_IGN);'
        'profile=Path(os.environ["LLVM_PROFILE_FILE_NAME"].replace("%4m","probe"));'
        'profile.write_bytes(b"counters");'
        'Path(sys.argv[1]).write_text(json.dumps({"pid":os.getpid(),"pgid":os.getpgrp(),"profile":str(profile)}));'
        'time.sleep(60)'
    )
    inner = (
        f'import sys;sys.path.insert(0,{str(SCRIPTS)!r});import suite_run as r;from pathlib import Path;'
        f'r.quality.ROOT=Path({str(tmp_path)!r});r.quality.OUTPUT=r.quality.ROOT/"reports";'
        'r.shutil.which=lambda _name:"fake-tool-no-cargo";'
        f'r.commands=lambda _s:([],[r.Gate(stage,(sys.executable,"-c",{child!r},str(Path({str(ready)!r})/stage)),'
        'env=r.quality.child_env(LLVM_PROFILE_FILE_NAME=r.quality.profile_file_name(stage))) for stage in ("unit","integration")]);'
        'sys.exit(r.main(["--full"]))'
    )
    outer = (
        f'import sys;sys.path.insert(0,{str(SCRIPTS)!r});import gate_and_fix as g;from coverage_run import Run;'
        'from gate_process import interrupt_signals,Interrupted;from pathlib import Path;'
        f'root=Path({str(tmp_path)!r})\n'
        'try:\n with interrupt_signals(),Run(root,root/"reports","gate"):\n'
        f'  g.run_gates([g.Gate("test-full",(sys.executable,"-c",{inner!r}))],cwd=root)\n'
        'except Interrupted as error:\n sys.exit(128+error.signum)\n'
    )
    env = {key: value for key, value in os.environ.items() if not key.startswith(('BUS_', 'HERDR_', 'CARGO_', 'LLVM_'))}
    env.update(CARGO_TARGET_DIR=str(tmp_path / 'normal'), CARGO_LLVM_COV_TARGET_DIR=str(tmp_path / 'coverage'))
    process = subprocess.Popen([sys.executable, '-c', outer], cwd=tmp_path, env=env,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
    identities = []
    try:
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline and process.poll() is None:
            if all((ready / stage).exists() for stage in ('unit', 'integration')):
                identities = [json.loads((ready / stage).read_text()) for stage in ('unit', 'integration')]
                break
            time.sleep(.02)
        assert len(identities) == 2, 'both workers must start'
        assert all(Path(item['profile']).is_file() for item in identities)
        process.send_signal(signum)
        _, stderr = process.communicate(timeout=12)
        assert process.returncode == 128 + signum, stderr
        assert not list((tmp_path / 'coverage').rglob('*.profraw'))
        assert not [item['pid'] for item in identities if running(item['pid'])], 'workers outlived profile cleanup'
    finally:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=5)
        for path in ready.iterdir():
            item = json.loads(path.read_text())
            if running(item['pid']) and os.getpgid(item['pid']) == item['pgid'] != os.getpgrp():
                try:
                    os.killpg(item['pgid'], signal.SIGKILL)
                except ProcessLookupError:
                    pass
