"""The restart harness must record a scratch-HOME decline before its first PTY."""
import json
import os
from pathlib import Path

import pytest

from tools.acceptance.harness_test import Bus


def test_acceptance_harness_prepares_current_bundle_without_installing_skills(tmp_path):
    binary = os.environ.get("BUS_TEST_BINARY")
    if not binary:
        pytest.skip("requires the gate's exact binary")
    home = tmp_path / "home"
    home.mkdir()
    work = tmp_path / "work"
    work.mkdir()
    env = {key: value for key, value in os.environ.items() if not key.startswith(("BUS_", "HERDR_", "XDG_"))}
    env.update(HOME=str(home), USERPROFILE=str(home), BUS_DATA_DIR=str(tmp_path / "data"))
    bus = Bus(Path(binary), tmp_path, env, work)
    bus.prepare_skills()
    saved = json.loads((home / ".local/share/bus/settings.json").read_text())
    assert saved["skills_answer"]["accepted"] is False
    assert saved["skills_answer"]["bundle_hash"]
    assert not (home / ".claude").exists()
    assert not (home / ".agents").exists()


def test_acceptance_setup_refuses_the_protected_home_before_creating_anything(tmp_path):
    protected = tmp_path / "protected"
    protected.mkdir()
    bus = Bus(Path("must-not-execute"), tmp_path,
              {"HOME": str(protected), "BUS_DATA_DIR": str(tmp_path / "data"), "TEST_BUS_PROTECTED_HOME": str(protected)})
    with pytest.raises(AssertionError, match="test isolation refused"):
        bus.prepare_skills()
    assert list(protected.iterdir()) == []
