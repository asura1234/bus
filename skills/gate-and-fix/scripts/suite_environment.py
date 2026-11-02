"""Prepare the checkout's private Python gate tools before scheduling lanes."""
from __future__ import annotations

from contextlib import contextmanager
import json
import os
from pathlib import Path
import subprocess
import sys

from coverage_run import EXTERNAL_RUN
from gate_process import Children

REQUIREMENTS = Path(__file__).with_name('requirements.txt')
TOOLS = ('pytest', 'xdist', 'ruff')
HEALTH = '''import importlib.util,json,sys
print(json.dumps(dict(prefix=sys.prefix,base=sys.base_prefix,tools={name:(importlib.util.find_spec(name).origin if importlib.util.find_spec(name) else None) for name in ('pytest','xdist','ruff')})))
'''


def private(path: Path, root: Path) -> Path:
    if not path.resolve().is_relative_to(root):
        raise ValueError(f'private Python environment path is outside the checkout: {path}')
    return path


@contextmanager
def setup_lock(path: Path):
    with path.open('a+b') as stream:
        if os.name == 'nt':
            import msvcrt
            if os.fstat(stream.fileno()).st_size == 0:
                stream.write(b'\0')
                stream.flush()
            stream.seek(0)
            try:
                msvcrt.locking(stream.fileno(), msvcrt.LK_NBLCK, 1)
            except OSError as error:
                raise ValueError(f'private Python environment setup is already running: {path}') from error
            try:
                yield
            finally:
                stream.seek(0)
                msvcrt.locking(stream.fileno(), msvcrt.LK_UNLCK, 1)
        else:
            import fcntl
            try:
                fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError as error:
                raise ValueError(f'private Python environment setup is already running: {path}') from error
            try:
                yield
            finally:
                fcntl.flock(stream, fcntl.LOCK_UN)


def command(argv: list[str], root: Path, log) -> str:
    env = {name: value for name, value in os.environ.items() if not name.startswith(('BUS_', 'HERDR_'))}
    children = Children(private_groups=not bool(os.environ.get(EXTERNAL_RUN)))
    log.write('+ ' + subprocess.list2cmdline(argv) + '\n')
    log.flush()
    process = children.start(argv, cwd=root, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                             text=True, encoding='utf-8', errors='replace')
    try:
        output, _ = process.communicate(timeout=120)
        log.write(output)
        log.flush()
        if process.returncode:
            raise ValueError(f'setup command exited {process.returncode}')
        return output
    except BaseException:
        children.stop()
        raise
    finally:
        children.finished(process)


def ensure_environment(root: Path, *, requirements: Path | None = None) -> Path:
    root = root.resolve()
    requirements = requirements or REQUIREMENTS
    if not requirements.is_file():
        raise ValueError(f'checked-in Python gate requirements are missing: {requirements}')
    directory = private(root / 'temp/gate-tools', root)
    prefix = private(directory / 'python', root)
    directory.mkdir(parents=True, exist_ok=True)
    log_path = private(directory / 'python-setup.log', root)
    lock = private(directory / 'python-setup.lock', root)
    python = prefix / ('Scripts/python.exe' if os.name == 'nt' else 'bin/python')
    try:
        with setup_lock(lock), log_path.open('w', encoding='utf-8') as log:
            config = private(prefix / 'pyvenv.cfg', root)
            if not python.is_file() or not config.is_file() or 'include-system-site-packages = false' not in config.read_text().lower():
                prefix.mkdir(mode=0o700, parents=True, exist_ok=True)
                base = str(Path(getattr(sys, '_base_executable', sys.executable)).resolve())
                command([base, '-I', '-m', 'venv', str(prefix)], root, log)
            command([str(python), '-I', '-m', 'pip', 'install', '--isolated', '--disable-pip-version-check',
                     '--no-input', '--no-cache-dir', '--retries', '0', '--timeout', '15',
                     '--requirement', str(requirements)], root, log)
            health = json.loads(command([str(python), '-I', '-c', HEALTH], root, log))
            if (Path(health['prefix']).resolve() != prefix.resolve() or health['prefix'] == health['base'] or any(
                    not health['tools'].get(name) or not Path(health['tools'][name]).resolve().is_relative_to(prefix.resolve())
                    for name in TOOLS)):
                raise ValueError('required gate tools are not installed in the private venv')
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        raise ValueError(f'cannot prepare private Python test environment at {prefix}: {error}; see {log_path}; no lanes started') from error
    return python


def bootstrap(root: Path, argv: list[str], *, script: Path) -> None:
    root = root.resolve()
    prefix = private(root / 'temp/gate-tools/python', root)
    config = private(prefix / 'pyvenv.cfg', root)
    was_isolated = config.is_file() and 'include-system-site-packages = false' in config.read_text().lower()
    python = ensure_environment(root)
    if (Path(sys.prefix).resolve() != prefix.resolve() or not was_isolated or
            any(name in os.environ for name in ('PYTHONHOME', 'PYTHONPATH'))):
        env = {name: value for name, value in os.environ.items() if name not in {'PYTHONHOME', 'PYTHONPATH'}}
        os.execve(str(python), [str(python), str(script), *argv], env)
