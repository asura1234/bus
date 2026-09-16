from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
RUN = REPO_ROOT / "run"


@unittest.skipUnless(os.name == "posix", "Bus launcher requires a POSIX host")
class RunLauncherTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory(prefix="bus-run-test-")
        self.root = Path(self.temp_dir.name)
        self.bin_dir = self.root / "bin"
        self.bin_dir.mkdir()
        self.cargo_args = self.root / "cargo-args"
        self.cargo_cwd = self.root / "cargo-cwd"
        self.herdr_args = self.root / "herdr-args"

        self.assertTrue(RUN.is_file(), "run launcher is missing")
        shutil.copy2(RUN, self.root / "run")
        (self.root / "run").chmod(0o755)
        self._write_executable(
            self.bin_dir / "cargo",
            """#!/bin/sh
printf '%s\\n' "$@" > "$FAKE_CARGO_ARGS"
printf '%s\\n' "$PWD" > "$FAKE_CARGO_CWD"
exit "${FAKE_CARGO_EXIT:-0}"
""",
        )
        self._write_executable(
            self.root / "target" / "debug" / "herdr",
            """#!/bin/sh
printf '%s\\n' "$@" > "$FAKE_HERDR_ARGS"
""",
        )

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    @staticmethod
    def _write_executable(path: Path, content: str) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
        path.chmod(0o755)

    def _run(self, *args: str, cargo_exit: int = 0) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [str(self.root / "run"), *args],
            cwd=self.root.parent,
            env={
                **os.environ,
                "PATH": f"{self.bin_dir}{os.pathsep}{os.environ['PATH']}",
                "FAKE_CARGO_ARGS": str(self.cargo_args),
                "FAKE_CARGO_CWD": str(self.cargo_cwd),
                "FAKE_CARGO_EXIT": str(cargo_exit),
                "FAKE_HERDR_ARGS": str(self.herdr_args),
            },
            capture_output=True,
            text=True,
            check=False,
        )

    def test_dev_rebuilds_from_the_repository_then_launches_bus_with_dev_and_arguments(self) -> None:
        result = self._run("dev", "resume", "--last")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            self.cargo_args.read_text(encoding="utf-8").splitlines(),
            ["build", "--locked", "--bin", "herdr"],
        )
        self.assertEqual(self.cargo_cwd.read_text(encoding="utf-8").strip(), str(self.root))
        self.assertEqual(
            self.herdr_args.read_text(encoding="utf-8").splitlines(),
            ["--bus", "--dev", "resume", "--last"],
        )

    def test_dev_forwards_the_orchestrator_control_flag_unchanged(self) -> None:
        result = self._run("dev", "--orchestrator-control", "resume", "--last")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            self.herdr_args.read_text(encoding="utf-8").splitlines(),
            ["--bus", "--dev", "--orchestrator-control", "resume", "--last"],
        )

    def test_dev_control_forwards_arguments_without_injecting_a_capability(self) -> None:
        result = self._run("dev-control", "state")

        self.assertEqual(result.returncode, 0, result.stderr)
        forwarded = self.herdr_args.read_text(encoding="utf-8")
        self.assertEqual(forwarded.splitlines(), ["--bus", "--dev", "state"])
        self.assertNotIn("BUS_ORCHESTRATOR_CONTROL_TOKEN", forwarded)

    def test_dev_never_launches_an_existing_binary_after_a_failed_build(self) -> None:
        result = self._run("dev", cargo_exit=17)

        self.assertEqual(result.returncode, 17)
        self.assertFalse(self.herdr_args.exists())

    def test_dev_control_uses_existing_binary_without_building(self) -> None:
        result = self._run("dev-control", "agent", "read", "3", "--source", "recent", "--lines", "5000")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.cargo_args.exists())
        self.assertEqual(
            self.herdr_args.read_text(encoding="utf-8").splitlines(),
            ["--bus", "--dev", "agent", "read", "3", "--source", "recent", "--lines", "5000"],
        )

    def test_dev_control_preserves_typed_permission_cli_shape_without_raw_keys(self) -> None:
        result = self._run(
            "dev-control",
            "agent",
            "approve-once",
            "3",
            "--fingerprint",
            "v1.bound.digest",
            "--response",
            "allow-once",
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.cargo_args.exists())
        self.assertEqual(
            self.herdr_args.read_text(encoding="utf-8").splitlines(),
            [
                "--bus",
                "--dev",
                "agent",
                "approve-once",
                "3",
                "--fingerprint",
                "v1.bound.digest",
                "--response",
                "allow-once",
            ],
        )
        self.assertNotIn("send-keys", self.herdr_args.read_text(encoding="utf-8"))

    def test_dev_control_fails_closed_when_debug_binary_is_missing(self) -> None:
        (self.root / "target" / "debug" / "herdr").unlink()
        result = self._run("dev-control", "state")

        self.assertEqual(result.returncode, 1)
        self.assertIn("debug binary is missing", result.stderr)
        self.assertFalse(self.cargo_args.exists())

    def test_old_build_dev_syntax_is_rejected_with_the_current_usage(self) -> None:
        result = self._run("build", "dev")

        self.assertEqual(result.returncode, 2)
        self.assertIn("./run dev [Bus arguments]", result.stderr)
        self.assertFalse(self.cargo_args.exists())


if __name__ == "__main__":
    unittest.main()
