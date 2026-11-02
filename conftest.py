import pytest


@pytest.fixture(autouse=True)
def isolate_git_config(tmp_path_factory, monkeypatch):
    config = tmp_path_factory.mktemp("git-config") / "global.gitconfig"
    config.write_text("")
    monkeypatch.setenv("GIT_CONFIG_GLOBAL", str(config))
    monkeypatch.setenv("GIT_CONFIG_NOSYSTEM", "1")
