"""A child remote check should confirm several SHAs without full-history transfers."""
import contextlib
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
import submodule_state  # noqa: E402


def git(repo, *args):
    return subprocess.run(['git', '-c', 'protocol.file.allow=always', '-C', str(repo), *args],
                          check=True, capture_output=True, text=True).stdout.strip()


def test_remote_confirmation_batches_shallow_shas_without_blob_history(tmp_path, monkeypatch):
    source = tmp_path / 'source'
    source.mkdir()
    git(source, 'init', '-q', '-b', 'main')
    for index in range(8):
        (source / f'blob{index}').write_bytes(os.urandom(2 * 1024 * 1024))
        git(source, 'add', f'blob{index}')
        git(source, '-c', 'user.name=t', '-c', 'user.email=t@t',
            'commit', '-qm', f'blob{index}')
    remote = tmp_path / 'child.git'
    git(tmp_path, 'clone', '-q', '--bare', str(source), str(remote))
    git(remote, 'config', 'uploadpack.allowFilter', 'true')
    child = tmp_path / 'child'
    git(tmp_path, 'clone', '-q', '--depth=2', remote.as_uri(), str(child))
    tip = git(child, 'rev-parse', 'HEAD')
    git(child, 'update-ref', '-d', 'refs/remotes/origin/main')
    assert git(child, 'rev-parse', '--is-shallow-repository') == 'true'
    assert len(git(child, 'log', '--format=%H', '--branches', 'HEAD',
                   '--not', '--remotes').splitlines()) == 2

    original_temp = tempfile.TemporaryDirectory
    sizes = []

    @contextlib.contextmanager
    def measured_temp(*args, **kwargs):
        with original_temp(*args, **kwargs) as directory:
            yield directory
            sizes.append(sum(path.stat().st_size for path in Path(directory).rglob('*')
                             if path.is_file()))

    monkeypatch.setattr(submodule_state, 'tempfile',
                        SimpleNamespace(TemporaryDirectory=measured_temp))
    assert submodule_state.unpublished_child_work(child, tip, tip) == ''
    assert len(sizes) == 1, f'expected one batched remote probe, got {len(sizes)}'
    assert sizes[0] < 512 * 1024, f'remote probe transferred {sizes[0]} bytes'
