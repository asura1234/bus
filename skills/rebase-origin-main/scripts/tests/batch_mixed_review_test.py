"""Remote confirmation should batch published and local-only child tips."""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import submodule_state
from child_publication_review_test import git, commit, setup


def test_mixed_publication_tips_use_at_most_one_remote_fetch(tmp_path, monkeypatch):
    _, _, parent, _, pin = setup(tmp_path)
    child = parent / 'child'
    git(child, 'update-ref', '-d', 'refs/remotes/origin/main')
    for name in ('local-one', 'local-two'):
        git(child, 'switch', '-q', '--detach', pin)
        git(child, 'switch', '-qc', name)
        commit(child, name)
    git(child, 'switch', '-q', '--detach', pin)
    calls = []
    original = submodule_state.fetch_remote_tips
    def counted(repo, remote, tips, **kwargs):
        calls.append(list(tips))
        return original(repo, remote, tips, **kwargs)
    monkeypatch.setattr(submodule_state, 'fetch_remote_tips', counted)
    result = submodule_state.unpublished_child_work(child, pin, pin)
    assert 'local-one' in result and 'local-two' in result
    assert len(calls) <= 1, f'expected at most one batched fetch, got {len(calls)}: {calls}'
