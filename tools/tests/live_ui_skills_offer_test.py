"""The live driver must reach the room screen in a HOME with no saved skills answer."""
import os
import shutil
import signal
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
pytest.importorskip("fcntl")
from tools.acceptance import live_ui_test as driver  # noqa: E402


def test_live_driver_reaches_the_room_screen_when_home_has_no_skills_answer(tmp_path, monkeypatch):
    if os.environ.get("BUS_TEST_BINARY"):
        monkeypatch.setattr(driver, "BINARY", Path(os.environ["BUS_TEST_BINARY"]))
    if not driver.BINARY.exists():
        pytest.skip("requires a built Bus binary")
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setenv("HOME", str(home))
    created, children = [], []
    make_temp, popen = driver.tempfile.mkdtemp, driver.subprocess.Popen
    def own_temp(*args, **kwargs):
        path = make_temp(*args, **kwargs)
        created.append(Path(path))
        return path
    def own_child(*args, **kwargs):
        child = popen(*args, **kwargs)
        children.append(child)
        return child
    monkeypatch.setattr(driver.tempfile, "mkdtemp", own_temp)
    monkeypatch.setattr(driver.subprocess, "Popen", own_child)
    try:
        bus = driver.LiveBus()
        try:
            assert "Install the following set of default skills?" not in bus.frame()
        finally:
            bus.stop()
    finally:
        for child in children:
            if child.poll() is None:
                os.killpg(child.pid, signal.SIGTERM)
                child.wait(timeout=5)
        for path in created:
            shutil.rmtree(path)
