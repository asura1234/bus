"""Read-only CLI coverage. Uses the built binary, never starts a Bus server."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class BusDevTests(unittest.TestCase):
    def test_control_never_enables_or_creates_a_missing_instance(self):
        with tempfile.TemporaryDirectory(prefix="bus-control-test-") as directory:
            root = Path(directory) / "absent"
            for prefix in [[], ["--dev"]]:
                result = subprocess.run(
                    [str(ROOT / "target/debug/herdr"), "--bus", *prefix, "state"],
                    env=dict(os.environ, BUS_DATA_DIR=str(root)), capture_output=True, text=True,
                    timeout=10, check=False,
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(json.loads(result.stdout)["ok"])
                self.assertFalse(root.exists())

    def test_invalid_control_mutations_fail_as_json_before_connecting(self):
        with tempfile.TemporaryDirectory(prefix="bus-control-test-") as directory:
            for args in [["room", "delete", "room"], ["send", "--room", "room", "--text", "hello"]]:
                result = subprocess.run(
                    [str(ROOT / "target/debug/herdr"), "--bus", *args],
                    env=dict(os.environ, BUS_DATA_DIR=directory), capture_output=True, text=True,
                    timeout=10, check=False,
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(json.loads(result.stdout)["error"]["code"], "invalid_arguments")
                self.assertEqual(list(Path(directory).iterdir()), [])

    def test_normal_paths_do_not_enable_dev_or_create_state(self):
        with tempfile.TemporaryDirectory(prefix="bus-normal-test-") as directory:
            result = subprocess.run(
                [str(ROOT / "target/debug/herdr"), "--bus", "--paths"],
                env=dict(os.environ, BUS_DATA_DIR=directory), capture_output=True, text=True,
                check=True,
            )
            self.assertFalse(json.loads(result.stdout)["dev"])
            self.assertFalse((Path(directory) / "state.json").exists())

    def test_dev_paths_accepts_both_orders_without_starting_a_session(self):
        with tempfile.TemporaryDirectory(prefix="bus-dev-test-") as directory:
            for args in [("--dev", "--paths"), ("--paths", "--dev")]:
                result = subprocess.run(
                    [str(ROOT / "target/debug/herdr"), "--bus", *args],
                    env=dict(os.environ, BUS_DATA_DIR=directory), capture_output=True, text=True,
                    check=False,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                paths = json.loads(result.stdout)
                self.assertTrue(paths["dev"])
                self.assertEqual(paths["data"], directory)
                self.assertIn("logs", paths)
                self.assertFalse((Path(directory) / "state.json").exists())

    def test_help_documents_dev_and_unknown_options_still_fail(self):
        result = subprocess.run([str(ROOT / "target/debug/herdr"), "--bus", "--help"], capture_output=True, text=True)
        self.assertIn("--dev", result.stdout)
        invalid = subprocess.run([str(ROOT / "target/debug/herdr"), "--bus", "--dev", "--unknown"], capture_output=True, text=True)
        self.assertNotEqual(invalid.returncode, 0)
