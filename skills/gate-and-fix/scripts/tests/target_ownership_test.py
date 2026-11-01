"""A lane cannot relink the default executable while another lane consumes it."""
from concurrent.futures import ThreadPoolExecutor, wait as real_wait
from dataclasses import replace
import os
from pathlib import Path
import sys
import time
from unittest.mock import patch

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import gate_and_fix as scheduler  # noqa: E402
import suite_run as runner  # noqa: E402
from suite_selection import Selection, select  # noqa: E402


def wait_for(path):
    deadline = time.monotonic() + 20
    while not path.exists() and time.monotonic() < deadline:
        time.sleep(.01)
    assert path.exists(), f'fixture did not reach {path.name}'


@pytest.mark.skipif(os.name != 'posix', reason='real executable launch fixture uses a POSIX shebang')
@pytest.mark.parametrize('consumer_name', ['unit', 'python', 'cli-smoke', 'affected-python'])
def test_default_target_rebuild_waits_for_executable_consumers(tmp_path, consumer_name):
    normal, coverage = tmp_path / 'normal', tmp_path / 'coverage'
    full = consumer_name != 'affected-python'
    selection = select([], full=True) if full else Selection(False, 'all()', ('tools',), False, 'mixed affected')
    with patch.dict(os.environ, CARGO_TARGET_DIR=str(normal), CARGO_LLVM_COV_TARGET_DIR=str(coverage)):
        _, commands = runner.commands(selection)
    gates = {gate.name: gate for gate in commands}
    consumer = gates['python' if consumer_name == 'affected-python' else consumer_name]
    mutator = gates['integration' if full else 'affected-rust']
    target = coverage if full else normal
    target.mkdir()
    binary = target / 'bus-test'
    binary.write_text('#!/bin/sh\nexit 0\n')
    binary.chmod(0o700)
    consume = (
        'import subprocess,sys,time\nfrom pathlib import Path\n'
        'root=Path(sys.argv[1]);binary=Path(sys.argv[2])\n'
        '(root/"started").touch()\n'
        'deadline=time.monotonic()+25\n'
        'while not (root/"release").exists() and time.monotonic()<deadline:time.sleep(.01)\n'
        'try: subprocess.run([str(binary)],check=True)\n'
        'finally: (root/"consumed").touch()\n'
    )
    rebuild = (
        'import sys,time\nfrom pathlib import Path\n'
        'root=Path(sys.argv[1]);binary=Path(sys.argv[2]);deadline=time.monotonic()+25\n'
        'while not (root/"started").exists() and time.monotonic()<deadline:time.sleep(.01)\n'
        'binary.unlink();(root/"unlinked").touch()\n'
        'while not (root/"consumed").exists() and time.monotonic()<deadline:time.sleep(.01)\n'
        'binary.write_text("#!/bin/sh\\nexit 0\\n");binary.chmod(0o700)\n'
    )
    submitted = {}

    class ObservedExecutor(ThreadPoolExecutor):
        def submit(self, function, gate, **kwargs):
            future = super().submit(function, gate, **kwargs)
            submitted[future] = gate.name
            return future

    released = False

    def controlled_wait(active, **kwargs):
        nonlocal released
        if not released:
            # First scheduling pass has admitted every non-conflicting lane.
            # Pause the real consumer until an admitted relinker has removed its
            # executable; if resource ownership prevented admission, release it.
            wait_for(target / 'started')
            if any(submitted[future] == mutator.name for future in active):
                wait_for(target / 'unlinked')
            (target / 'release').touch()
            released = True
        return real_wait(active, **kwargs)

    with patch.object(scheduler, 'ThreadPoolExecutor', ObservedExecutor), patch.object(scheduler, 'wait', controlled_wait):
        results = scheduler.run_gates([
            replace(consumer, argv=(sys.executable, '-c', consume, str(target), str(binary))),
            replace(mutator, argv=(sys.executable, '-c', rebuild, str(target), str(binary))),
        ], cwd=tmp_path)
    assert [result.exit_code for result in results] == [0, 0], '\n'.join(r.stderr for r in results)
