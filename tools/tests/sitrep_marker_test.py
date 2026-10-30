"""A room marker advances only after a valid report window is accepted."""
import json
import subprocess
import sys
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[2] / "skills/sitrep/scripts/marker.py"


def call(tmp_path, command, payload=None, room="568"):
    return subprocess.run([sys.executable, str(SCRIPT), command, "--main", str(tmp_path), "--room", room, "--now", "2026-10-30T13:39:35Z"], input=json.dumps(payload) if payload is not None else None, text=True, capture_output=True, check=False)


def test_first_window_and_advance(tmp_path):
    read = call(tmp_path, "read")
    assert read.returncode == 0, read.stderr
    window = json.loads(read.stdout)
    assert window == {"since": "2026-10-29T13:39:35Z", "cutoff": "2026-10-30T13:39:35Z", "initial": True, "items": [], "revision": None}
    assert not (tmp_path / "temp").exists()
    saved = call(tmp_path, "write", {"timestamp": window["cutoff"], "items": ["pr:214"], "revision": window["revision"]})
    assert saved.returncode == 0, saved.stderr
    assert json.loads((tmp_path / "temp/.bus/sitrep-568.json").read_text()) == {"timestamp": window["cutoff"], "items": ["pr:214"]}
    next_window = json.loads(call(tmp_path, "read").stdout)
    assert next_window["since"] == window["cutoff"] and next_window["items"] == ["pr:214"]
    assert next_window["initial"] is False and next_window["revision"]


def test_malformed_marker_never_resets_and_unsafe_room_is_rejected(tmp_path):
    marker = tmp_path / "temp/.bus/sitrep-568.json"
    marker.parent.mkdir(parents=True)
    marker.write_text('{"timestamp":"bad","items":[]}')
    result = call(tmp_path, "read")
    assert result.returncode != 0 and not result.stdout
    assert marker.read_text() == '{"timestamp":"bad","items":[]}'
    assert call(tmp_path, "read", room="../outside").returncode != 0


def test_stale_marker_update_is_not_overwritten(tmp_path):
    window = json.loads(call(tmp_path, "read").stdout)
    payload = {"timestamp": window["cutoff"], "items": ["first"], "revision": window["revision"]}
    assert call(tmp_path, "write", payload).returncode == 0
    payload["items"] = ["stale"]
    result = call(tmp_path, "write", payload)
    assert result.returncode != 0
    assert json.loads((tmp_path / "temp/.bus/sitrep-568.json").read_text())["items"] == ["first"]


def test_future_marker_and_escaping_directory_are_rejected(tmp_path):
    marker = tmp_path / "temp/.bus/sitrep-568.json"
    marker.parent.mkdir(parents=True)
    marker.write_text('{"timestamp":"2026-10-31T13:39:35Z","items":[]}')
    result = call(tmp_path, "read")
    assert result.returncode != 0 and not result.stdout
    marker.unlink()
    marker.parent.rmdir()
    outside = tmp_path.parent / (tmp_path.name + "-outside")
    outside.mkdir()
    marker.parent.symlink_to(outside, target_is_directory=True)
    result = call(tmp_path, "read")
    assert result.returncode != 0 and not result.stdout
