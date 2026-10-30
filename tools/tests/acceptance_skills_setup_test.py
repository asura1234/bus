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
