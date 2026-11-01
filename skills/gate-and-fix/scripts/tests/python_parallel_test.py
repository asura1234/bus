"""The Python lane keeps every selected case while running independent files together."""
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
from unittest.mock import patch

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import suite_run as runner  # noqa: E402
from suite_selection import Selection  # noqa: E402


def python_gate(root, workers):
    selection = Selection(False, 'none()', ('tools',), False, 'fixture')
    with patch.object(runner.quality, 'ROOT', root), patch.dict(os.environ, PYTEST_WORKERS=str(workers)):
        build, tests = runner.commands(selection)
    assert not build
    return tests[0]


def test_selected_files_overlap_without_splitting_module_fixtures(tmp_path):
    tools = tmp_path / 'tools'
    tools.mkdir()
    for name in ('first', 'second'):
        (tools / f'{name}_test.py').write_text(
            'import os,time\nfrom pathlib import Path\nimport pytest\n'
            f'ROOT=Path({str(tmp_path)!r})\nNAME={name!r}\n'
            '@pytest.fixture(scope="module",autouse=True)\ndef once():\n'
            '    with (ROOT/(NAME+".fixture")).open("x") as stream: stream.write(str(os.getpid()))\n'
            'def test_meet():\n'
            '    (ROOT/(NAME+".start")).write_text(str(os.getpid()))\n'
            '    deadline=time.monotonic()+5\n'
            '    while len(list(ROOT.glob("*.start")))<2 and time.monotonic()<deadline: time.sleep(.01)\n'
            '    assert len(list(ROOT.glob("*.start")))==2, "selected files must overlap"\n'
            'def test_once():\n'
            '    (ROOT/(NAME+".done")).open("x").close()\n'
        )
    (tmp_path / 'skills').mkdir()
    (tmp_path / 'skills/unselected_test.py').write_text('def test_unselected(): assert False\n')
    result = subprocess.run(python_gate(tmp_path, 2).argv, cwd=tmp_path,
                            capture_output=True, text=True, timeout=25)
    assert result.returncode == 0, result.stdout + result.stderr
    assert '4 passed' in result.stdout
    assert len({path.read_text() for path in tmp_path.glob('*.start')}) == 2
    assert len(list(tmp_path.glob('*.fixture'))) == len(list(tmp_path.glob('*.done'))) == 2


@pytest.mark.parametrize(('file_count', 'requested', 'expected'), [(1, 4, 0), (2, 4, 2),
                                                                (2, 8, 2), (5, 4, 4)])
def test_worker_processes_match_selected_file_parallelism(tmp_path, file_count, requested, expected):
    tools = tmp_path / 'tools'
    tools.mkdir()
    (tools / 'conftest.py').write_text(
        'from pathlib import Path\n'
        'def pytest_sessionstart(session):\n'
        '    if hasattr(session.config,"workerinput"):\n'
        f'        (Path({str(tmp_path)!r})/("worker-"+session.config.workerinput["workerid"])).touch()\n'
    )
    for index in range(file_count):
        (tools / f'case_{index}_test.py').write_text('def test_selected(): pass\n')
    result = subprocess.run(python_gate(tmp_path, requested).argv, cwd=tmp_path,
                            env=runner.quality.child_env(PYTEST_ADDOPTS='-n 6'),
                            capture_output=True, text=True, timeout=25)
    assert result.returncode == 0, result.stdout + result.stderr
    assert f'{file_count} passed' in result.stdout
    assert len(list(tmp_path.glob('worker-*'))) == expected


@pytest.mark.parametrize('workers', ['-1', 'auto', '1.5'])
def test_invalid_worker_count_fails_before_launch(tmp_path, workers):
    (tmp_path / 'tools').mkdir()
    (tmp_path / 'tools/example_test.py').write_text('def test_example(): pass\n')
    with pytest.raises(ValueError, match='PYTEST_WORKERS'):
        python_gate(tmp_path, workers)


def test_serial_override_keeps_the_same_test_inventory(tmp_path):
    (tmp_path / 'tools').mkdir()
    (tmp_path / 'tools/example_test.py').write_text('def test_first(): pass\ndef test_second(): pass\n')
    (tmp_path / 'tools/other_test.py').write_text('def test_third(): pass\n')
    for workers in (0, 2):
        result = subprocess.run(python_gate(tmp_path, workers).argv, cwd=tmp_path,
                                capture_output=True, text=True, timeout=20)
        assert result.returncode == 0, result.stdout + result.stderr
        assert '3 passed' in result.stdout


def test_serial_override_wins_over_inherited_pytest_worker_options(tmp_path):
    (tmp_path / 'tools').mkdir()
    (tmp_path / 'tools/example_test.py').write_text(
        'def test_serial(request): assert not hasattr(request.config,"workerinput")\n'
    )
    env = runner.quality.child_env(PYTEST_ADDOPTS='-n 2')
    result = subprocess.run(python_gate(tmp_path, 0).argv, cwd=tmp_path, env=env,
                            capture_output=True, text=True, timeout=20)
    assert result.returncode == 0, result.stdout + result.stderr


@pytest.mark.parametrize(('source', 'passed'), [('', True), ('def test_bad(): assert False\n', False),
                                                ('raise RuntimeError("bad collection")\n', False)])
def test_parallel_empty_failure_and_collection_error_remain_distinct(tmp_path, source, passed):
    (tmp_path / 'tools').mkdir()
    (tmp_path / 'tools/example_test.py').write_text(source)
    (tmp_path / 'tools/other_test.py').write_text('')
    with patch.object(runner.quality, 'ROOT', tmp_path):
        results = runner.batch([python_gate(tmp_path, 2)])
    assert results[0].passed is passed


@pytest.mark.skipif(os.name != 'posix', reason='POSIX process-group teardown')
@pytest.mark.parametrize('nested', [False, True])
def test_interrupt_stops_xdist_workers_and_their_descendants(tmp_path, nested):
    tools = tmp_path / 'tools'
    tools.mkdir()
    (tools / 'other_test.py').write_text('def test_other(): pass\n')
    (tools / 'conftest.py').write_text(
        'import os\nfrom pathlib import Path\n'
        'def pytest_sessionstart(session):\n'
        '    if hasattr(session.config,"workerinput"):\n'
        f'        ready=Path({str(tmp_path)!r},session.config.workerinput["workerid"])\n'
        '        pending=ready.with_name("."+ready.name+".tmp")\n'
        '        pending.write_text(str(os.getpid()));pending.replace(ready)\n'
    )
    child = (
        'import os,signal,time;from pathlib import Path;'
        'signal.signal(signal.SIGTERM,signal.SIG_IGN);'
        f'ready=Path({str(tmp_path / "child")!r});pending=ready.with_suffix(".tmp");'
        'pending.write_text(str(os.getpid()));pending.replace(ready);time.sleep(60)'
    )
    (tools / 'example_test.py').write_text(
        'import subprocess,sys,time\nfrom pathlib import Path\n'
        'def test_wait():\n'
        f'    subprocess.Popen([sys.executable,"-c",{child!r}])\n'
        '    time.sleep(60)\n'
    )
    scripts = str(Path(runner.__file__).parent)
    script = (
        f'import sys;sys.path.insert(0,{scripts!r});import suite_run as r;from pathlib import Path;'
        'from gate_process import interrupt_signals,Interrupted;'
        f'r.quality.ROOT=Path({str(tmp_path)!r})\n'
        'try:\n'
        '    with interrupt_signals():\n'
        '        _,tests=r.commands(r.Selection(False,"none()",("tools",),False,"fixture"))\n'
        f'        r.batch(tests,nested={nested!r})\n'
        'except Interrupted as error: sys.exit(128+error.signum)\n'
    )
    if nested:
        # The canonical gate owns the outer group; the Python lane inherits it.
        script = (
            f'import sys;sys.path.insert(0,{scripts!r});import gate_and_fix as g;'
            'from gate_process import interrupt_signals,Interrupted\n'
            'try:\n'
            '    with interrupt_signals():\n'
            f'        g.run_gates([g.Gate("test-full",(sys.executable,"-c",{script!r}))],cwd=g.Path({str(tmp_path)!r}))\n'
            'except Interrupted as error: sys.exit(128+error.signum)\n'
        )
    env = runner.quality.child_env(PYTEST_WORKERS='2')
    process = subprocess.Popen([sys.executable, '-c', script], cwd=tmp_path, env=env,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    pids = []
    try:
        deadline = time.monotonic() + 15
        while not (tmp_path / 'child').exists() and process.poll() is None and time.monotonic() < deadline:
            time.sleep(.02)
        assert (tmp_path / 'child').exists(), process.communicate(timeout=1)
        pids = [int(path.read_text()) for path in [*tmp_path.glob('gw*'), tmp_path / 'child']]
        assert len(pids) == 3
        process.send_signal(signal.SIGTERM)
        stdout, stderr = process.communicate(timeout=10)
        assert process.returncode == 128 + signal.SIGTERM, stdout + stderr
        for pid in pids:
            state = subprocess.run(['ps', '-p', str(pid), '-o', 'stat='], capture_output=True, text=True)
            assert not state.stdout.strip() or state.stdout.strip().startswith('Z'), (pid, state.stdout)
    finally:
        if process.poll() is None:
            process.send_signal(signal.SIGTERM)
            try:
                process.communicate(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        for pid in pids:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass


@pytest.mark.parametrize("workers", [0, 2])
def test_missing_parallel_dependency_fails_before_launch(tmp_path, workers):
    (tmp_path / 'tools').mkdir()
    (tmp_path / 'tools/example_test.py').write_text('def test_example(): pass\n')
    with patch.object(runner.quality.importlib.util, 'find_spec', return_value=None):
        with pytest.raises(ValueError, match='requirements.txt'):
            python_gate(tmp_path, workers)
