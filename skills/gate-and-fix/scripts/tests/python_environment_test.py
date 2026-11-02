"""Real private venv setup, using local test wheels instead of the network."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import zipfile

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import suite_environment as environment


def wheel(root, distribution, module, version='0.0.1'):
    normalized=distribution.replace('-','_')
    dist=f'{normalized}-{version}.dist-info'
    files={f'{module}/__init__.py':f'__version__={version!r}\n',
           f'{dist}/METADATA':f'Metadata-Version: 2.1\nName: {distribution}\nVersion: {version}\n',
           f'{dist}/WHEEL':'Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n'}
    files[f'{dist}/RECORD']=''.join(f'{name},,\n' for name in [*files,f'{dist}/RECORD'])
    with zipfile.ZipFile(root/f'{normalized}-{version}-py3-none-any.whl','w') as archive:
        for name,text in files.items():archive.writestr(name,text)


def requirements(root, *, xdist=True, version='0.0.1'):
    wheels=root/'wheels';wheels.mkdir(exist_ok=True)
    for distribution,module in [('pytest','pytest'),('pytest-xdist','xdist'),('ruff','ruff')]:
        wheel(wheels,distribution,module,version)
    path=root/'requirements.txt'
    names=['pytest','ruff']+(['pytest-xdist'] if xdist else [])
    path.write_text('--no-index\n--find-links '+str(wheels)+'\n'+''.join(f'{name}=={version}\n' for name in names))
    return path


def inspect(python):
    script='import json,sys,pytest,xdist,ruff;print(json.dumps(dict(prefix=sys.prefix,base=sys.base_prefix,files=[m.__file__ for m in (pytest,xdist,ruff)],versions=[m.__version__ for m in (pytest,xdist,ruff)])))'
    return json.loads(subprocess.check_output([str(python),'-I','-c',script],text=True))


def test_fresh_environment_installs_checked_requirements_and_reuses_packages(tmp_path):
    req=requirements(tmp_path)
    python=environment.ensure_environment(tmp_path,requirements=req)
    info=inspect(python)
    prefix=tmp_path/'temp/gate-tools/python'
    assert Path(info['prefix']).resolve()==prefix.resolve()
    assert info['prefix']!=info['base']
    assert all(Path(name).resolve().is_relative_to(prefix.resolve()) for name in info['files'])
    before={name:Path(name).stat().st_mtime_ns for name in info['files']}
    assert environment.ensure_environment(tmp_path,requirements=req)==python
    assert before=={name:Path(name).stat().st_mtime_ns for name in info['files']}


def test_existing_environment_missing_xdist_is_repaired_and_requirements_updates_apply(tmp_path):
    prefix=tmp_path/'temp/gate-tools/python'
    subprocess.run([sys.executable,'-m','venv',str(prefix)],check=True,capture_output=True)
    python=prefix/('Scripts/python.exe' if os.name=='nt' else 'bin/python')
    req=requirements(tmp_path,xdist=False)
    subprocess.run([str(python),'-m','pip','install','--isolated','--disable-pip-version-check','--no-cache-dir','-r',str(req)],check=True,capture_output=True)
    assert subprocess.run([str(python),'-I','-c','import xdist'],capture_output=True).returncode!=0
    req=requirements(tmp_path)
    assert inspect(environment.ensure_environment(tmp_path,requirements=req))['versions']==['0.0.1']*3
    req=requirements(tmp_path,version='0.0.2')
    assert inspect(environment.ensure_environment(tmp_path,requirements=req))['versions']==['0.0.2']*3


def test_impossible_install_fails_clearly_without_using_global_environment(tmp_path):
    req=requirements(tmp_path)
    with req.open('a') as stream:stream.write('package-that-is-not-in-the-local-wheelhouse==0.0.1\n')
    with pytest.raises(ValueError,match='private Python test environment.*python-setup.log'):
        environment.ensure_environment(tmp_path,requirements=req)
    log=tmp_path/'temp/gate-tools/python-setup.log'
    assert 'package-that-is-not-in-the-local-wheelhouse' in log.read_text()


def test_environment_path_cannot_escape_the_checkout(tmp_path):
    outside=tmp_path/'outside';outside.mkdir()
    repo=tmp_path/'repo';repo.mkdir()
    (repo/'temp').symlink_to(outside,target_is_directory=True)
    with pytest.raises(ValueError,match='outside'):
        environment.ensure_environment(repo,requirements=requirements(tmp_path))
    assert list(outside.iterdir())==[]


def test_inherited_pip_configuration_cannot_redirect_private_installation(tmp_path, monkeypatch):
    repo=tmp_path/'repo';repo.mkdir()
    outside=tmp_path/'outside';outside.mkdir()
    config=tmp_path/'pip.conf'
    config.write_text('[install]\ntarget = '+str(outside)+'\n')
    monkeypatch.setenv('PIP_CONFIG_FILE',str(config))
    monkeypatch.setenv('PIP_TARGET',str(outside))
    python=None
    try:
        python=environment.ensure_environment(repo,requirements=requirements(tmp_path))
    finally:
        assert list(outside.iterdir())==[], 'setup must not write outside the private environment'
    info=inspect(python)
    assert all(Path(name).resolve().is_relative_to((repo/'temp/gate-tools/python').resolve()) for name in info['files'])


def test_setup_timeout_preserves_emitted_stdout_and_stderr(tmp_path, monkeypatch):
    ready=tmp_path/'ready'
    script=('import sys,time;from pathlib import Path;'
            'print("setup-before-timeout",flush=True);'
            'print("setup-stderr",file=sys.stderr,flush=True);'
            f'Path({str(ready)!r}).touch();time.sleep(30)')
    communicate=subprocess.Popen.communicate
    def short_deadline(process,*args,**kwargs):
        deadline=time.monotonic()+5
        while not ready.exists():
            assert time.monotonic()<deadline,'fixture did not emit its diagnostics'
            time.sleep(.01)
        kwargs['timeout']=.05
        return communicate(process,*args,**kwargs)
    monkeypatch.setattr(subprocess.Popen,'communicate',short_deadline)
    log=tmp_path/'python-setup.log'
    with log.open('w') as stream,pytest.raises(subprocess.TimeoutExpired):
        environment.command([sys.executable,'-c',script],tmp_path,stream)
    assert 'setup-before-timeout' in log.read_text().splitlines()
    assert 'setup-stderr' in log.read_text().splitlines()


def test_bootstrap_reexecs_the_private_interpreter_and_preserves_arguments(tmp_path):
    req=requirements(tmp_path)
    entry=tmp_path/'entry.py'
    entry.write_text('import json,sys;print(json.dumps(dict(prefix=sys.prefix,args=sys.argv[1:])))')
    script=(f'import sys;from pathlib import Path;sys.path.insert(0,{str(Path(environment.__file__).parent)!r});'
            f'import suite_environment as e;e.REQUIREMENTS=Path({str(req)!r});'
            f'e.bootstrap(Path({str(tmp_path)!r}),["--full","value with spaces"],script=Path({str(entry)!r}))')
    result=subprocess.run([sys.executable,'-c',script],text=True,capture_output=True,check=True)
    info=json.loads(result.stdout)
    assert Path(info['prefix']).resolve()==(tmp_path/'temp/gate-tools/python').resolve()
    assert info['args']==['--full','value with spaces']


def test_bootstrap_reexecs_after_migrating_a_system_site_venv(tmp_path):
    req=requirements(tmp_path)
    prefix=tmp_path/'temp/gate-tools/python'
    subprocess.run([sys.executable,'-m','venv','--system-site-packages',str(prefix)],check=True,capture_output=True)
    python=prefix/('Scripts/python.exe' if os.name=='nt' else 'bin/python')
    entry=tmp_path/'entry.py'
    entry.write_text('import json,sys;print(json.dumps(dict(prefix=sys.prefix,paths=sys.path)))')
    script=(f'import sys;from pathlib import Path;sys.path.insert(0,{str(Path(environment.__file__).parent)!r});'
            f'import suite_environment as e;e.REQUIREMENTS=Path({str(req)!r});'
            f'e.bootstrap(Path({str(tmp_path)!r}),[],script=Path({str(entry)!r}))')
    result=subprocess.run([str(python),'-c',script],text=True,capture_output=True,check=True)
    assert result.stdout, 'the migrated parent must re-exec the entrypoint'
    info=json.loads(result.stdout)
    assert not [name for name in info['paths'] if 'site-packages' in name and not Path(name).resolve().is_relative_to(prefix.resolve())]


@pytest.mark.parametrize('entrypoint',['suite','diagnostic'])
def test_setup_failure_prevents_any_lane_from_starting(tmp_path,entrypoint):
    from unittest.mock import patch
    import bus_quality as quality
    import suite_run as runner
    with patch.object(quality,'ROOT',tmp_path):
        if entrypoint=='suite':
            with (patch.object(runner,'changed_paths',return_value=['run']),
                  patch.object(runner,'bootstrap',side_effect=ValueError('private setup impossible'),create=True),
                  patch.object(runner,'needs_profiles',return_value=False),
                  patch.object(runner.shutil,'which',return_value='tool'),
                  patch.object(runner,'execute',return_value=0) as execute):
                assert runner.main(['--full'])==1
            execute.assert_not_called()
        else:
            with (patch.object(quality,'bootstrap',side_effect=ValueError('private setup impossible'),create=True),
                  patch.object(quality,'run_lane',return_value=0) as lane):
                assert quality.main(['lint'])==1
            lane.assert_not_called()


def test_plan_does_not_prepare_or_modify_the_python_environment(tmp_path):
    from unittest.mock import patch
    import suite_run as runner
    with (patch.object(runner.quality,'ROOT',tmp_path),patch.object(runner.quality,'head',return_value='head'),
          patch.object(runner,'changed_paths',return_value=['run']),
          patch.object(runner,'bootstrap',side_effect=AssertionError('plan must stay read-only'),create=True) as bootstrap):
        assert runner.main(['--full','--plan'])==0
    bootstrap.assert_not_called()
    assert not (tmp_path/'temp').exists()
