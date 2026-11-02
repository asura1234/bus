"""Caps apply to both entrypoints and prevent simultaneous heavy test lanes."""
from concurrent.futures import ThreadPoolExecutor, wait as real_wait
from dataclasses import replace
import os
from pathlib import Path
import sys
from unittest.mock import patch

import pytest

sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
import bus_quality as quality
import gate_and_fix as scheduler
import suite_run as runner
from suite_selection import select


def test_full_entrypoint_caps_explicit_rust_and_python_overrides(tmp_path):
    tools=tmp_path/'tools';tools.mkdir()
    for number in range(6):(tools/f'case{number}_test.py').write_text('def test_case(): pass\n')
    with patch.dict(os.environ,NEXTEST_TEST_THREADS='99',PYTEST_WORKERS='99'),patch.object(quality,'ROOT',tmp_path):
        build,tests=runner.commands(select(['run'],full=True,root=tmp_path))
    gates={gate.name:gate for gate in [*build,*tests]}
    for name in ('instrumented-build','unit','integration','dev-tools'):
        assert name in gates
        argv=gates[name].argv
        assert '--test-threads' in argv
        assert argv[argv.index('--test-threads')+1]=='4'
    assert 'python' in gates
    assert gates['python'].argv[gates['python'].argv.index('-n')+1]=='4'


@pytest.mark.parametrize('nextest',[True,False])
def test_legacy_devtools_entrypoint_caps_every_batch(nextest):
    with patch.object(quality.shutil,'which',return_value='tool' if nextest else None),patch.object(quality,'run',return_value=0) as run,patch.dict(os.environ,NEXTEST_TEST_THREADS='99'):
        assert quality.dev_tools_test()==0
    for call in run.call_args_list:
        argv=call.args
        assert '--test-threads' in argv
        assert argv[argv.index('--test-threads')+1]=='4'
        if not nextest:assert argv.index('--')<argv.index('--test-threads')


@pytest.mark.parametrize('nextest',[True,False])
@pytest.mark.parametrize('server',[None,False,True])
def test_legacy_default_rust_batches_use_the_same_worker_cap(nextest,server):
    with patch.object(quality.shutil,'which',return_value='tool' if nextest else None),patch.object(quality,'run',return_value=0) as run,patch.dict(os.environ,NEXTEST_TEST_THREADS='99'):
        assert quality.rust_test('--bin','bus',in_process_server=server)==0
    argv=run.call_args.args
    assert argv[argv.index('--test-threads')+1]=='4'
    if not nextest:
        assert argv.index('--')<argv.index('--test-threads')
        if server is False:assert argv[-2:]==('--skip',quality.IN_PROCESS_SERVER_TESTS)


@pytest.mark.parametrize('value',['0','-1','many'])
def test_invalid_rust_worker_setting_fails_before_launch(value):
    with patch.dict(os.environ,NEXTEST_TEST_THREADS=value),pytest.raises(ValueError,match='positive integer'):
        runner.nextest('--bin','bus')


def test_gate_does_not_admit_devtools_while_unit_workers_are_active(tmp_path):
    _,tests=runner.commands(select(['run'],full=True))
    chosen=[gate for gate in tests if gate.name in {'unit','dev-tools'}]
    script='import sys,time;from pathlib import Path;p=Path(sys.argv[1]);p.with_suffix(".started").touch()\nwhile not p.exists():time.sleep(.01)\n'
    gates=[replace(gate,argv=(sys.executable,'-c',script,str(tmp_path/'release'))) for gate in chosen]
    submitted={}
    class ObservedExecutor(ThreadPoolExecutor):
        def submit(self,function,gate,**kwargs):
            future=super().submit(function,gate,**kwargs);submitted[future]=gate.name;return future
    checked=False
    def observed_wait(active,**kwargs):
        nonlocal checked
        if not checked:
            names={submitted[future] for future in active}
            (tmp_path/'release').touch()
            checked=True
            assert names=={'unit'},f'concurrent direct test lanes: {names}'
        return real_wait(active,**kwargs)
    with patch.object(scheduler,'ThreadPoolExecutor',ObservedExecutor),patch.object(scheduler,'wait',observed_wait):
        results=scheduler.run_gates(gates,cwd=tmp_path)
    assert all(result.passed for result in results)
