"""Acceptance setup must not install skills or touch the developer's HOME."""
import os
from pathlib import Path

import pytest

from tools.acceptance.harness_test import Bus


def test_acceptance_harness_does_not_install_skills_or_write_a_decline(tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    work = tmp_path / "work"
    work.mkdir()
    env = {key: value for key, value in os.environ.items() if not key.startswith(("BUS_", "HERDR_", "XDG_"))}
    env.update(HOME=str(home), USERPROFILE=str(home), BUS_DATA_DIR=str(tmp_path / "data"))
    bus = Bus(Path("must-not-execute"), tmp_path, env, work)
    bus.prepare_skills()
    assert list(home.iterdir()) == []


def test_acceptance_setup_refuses_the_protected_home_before_creating_anything(tmp_path):
    protected = tmp_path / "protected"
    protected.mkdir()
    bus = Bus(Path("must-not-execute"), tmp_path,
              {"HOME": str(protected), "BUS_DATA_DIR": str(tmp_path / "data"), "TEST_BUS_PROTECTED_HOME": str(protected)})
    with pytest.raises(AssertionError, match="test isolation refused"):
        bus.prepare_skills()
    assert list(protected.iterdir()) == []


@pytest.mark.parametrize("kind", ["public", "live", "relative"])
def test_real_provider_login_home_still_requires_private_non_live_bus_data(tmp_path, monkeypatch, kind):
    home = tmp_path / "login-home"
    home.mkdir()
    monkeypatch.setenv("HOME", str(home))
    data = home / ".local/share/bus" if kind == "live" else tmp_path / "data"
    data.mkdir(parents=True, mode=0o755 if kind == "public" else 0o700)
    env = {"HOME": str(home), "PATH": "/usr/bin:/bin",
           "BUS_DATA_DIR": "relative" if kind == "relative" else str(data)}
    bus = Bus(Path("must-not-execute"), tmp_path, env, real_providers=True)
    with pytest.raises(AssertionError, match="private 0700 BUS_DATA_DIR"):
        bus.prepare_skills()
