from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
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
        self.bus_args = self.root / "bus-args"

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
            self.root / "target" / "debug" / "bus",
            """#!/bin/sh
printf '%s\\n' "$@" > "$FAKE_BUS_ARGS"
""",
        )
        self._write_executable(
            self.root / "target" / "release" / "bus",
            """#!/bin/sh
{ echo release; printf '%s\\n' "$@"; } > "$FAKE_BUS_ARGS"
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
                # The launcher follows an inherited target dir (a gate run sets
                # one), which would bypass this fixture's fake binaries.
                **{
                    key: value
                    for key, value in os.environ.items()
                    if key not in ("CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR")
                },
                "PATH": f"{self.bin_dir}{os.pathsep}{os.environ['PATH']}",
                "FAKE_CARGO_ARGS": str(self.cargo_args),
                "FAKE_CARGO_CWD": str(self.cargo_cwd),
                "FAKE_CARGO_EXIT": str(cargo_exit),
                "FAKE_BUS_ARGS": str(self.bus_args),
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
            ["build", "--locked", "--bin", "bus", "--features", "dev-tools"],
        )
        self.assertEqual(self.cargo_cwd.read_text(encoding="utf-8").strip(), str(self.root))
        self.assertEqual(
            self.bus_args.read_text(encoding="utf-8").splitlines(),
            ["--dev", "resume", "--last"],
        )

    def test_prod_builds_the_release_variant_without_dev_tools_then_launches_it(self) -> None:
        result = self._run("prod", "resume", "--last")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            self.cargo_args.read_text(encoding="utf-8").splitlines(),
            ["build", "--locked", "--release", "--bin", "bus"],
        )
        self.assertEqual(
            self.bus_args.read_text(encoding="utf-8").splitlines(),
            ["release", "resume", "--last"],
        )

    def test_prod_never_launches_an_existing_binary_after_a_failed_build(self) -> None:
        result = self._run("prod", cargo_exit=9)

        self.assertEqual(result.returncode, 9)
        self.assertFalse(self.bus_args.exists())

    def test_prod_launches_the_fresh_binary_from_a_custom_cargo_target_dir(self) -> None:
        target_dir = self.root / "isolated-build"
        self._write_executable(
            self.bin_dir / "cargo",
            """#!/bin/sh
set -eu
mkdir -p "$CARGO_TARGET_DIR/release"
cat > "$CARGO_TARGET_DIR/release/bus" <<'BUS'
#!/bin/sh
{ echo fresh-release; printf '%s\\n' "$@"; } > "$FAKE_BUS_ARGS"
BUS
chmod +x "$CARGO_TARGET_DIR/release/bus"
""",
        )
        result = subprocess.run(
            [str(self.root / "run"), "prod", "--paths"],
            cwd=self.root.parent,
            env={
                **os.environ,
                "PATH": f"{self.bin_dir}{os.pathsep}{os.environ['PATH']}",
                "CARGO_TARGET_DIR": str(target_dir),
                "FAKE_BUS_ARGS": str(self.bus_args),
            },
            capture_output=True,
            text=True,
            check=False,
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((target_dir / "release" / "bus").is_file())
        self.assertEqual(
            self.bus_args.read_text(encoding="utf-8").splitlines(),
            ["fresh-release", "--paths"],
            "prod must launch what Cargo just built, not a stale default-target binary",
        )

    def test_dev_never_launches_an_existing_binary_after_a_failed_build(self) -> None:
        result = self._run("dev", cargo_exit=17)

        self.assertEqual(result.returncode, 17)
        self.assertFalse(self.bus_args.exists())

    def test_dev_control_uses_existing_binary_without_building(self) -> None:
        result = self._run("dev-control", "agent", "read", "3", "--source", "recent", "--lines", "5000")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.cargo_args.exists())
        self.assertEqual(
            self.bus_args.read_text(encoding="utf-8").splitlines(),
            ["--dev", "agent", "read", "3", "--source", "recent", "--lines", "5000"],
        )

    def test_dev_control_preserves_typed_dialog_cli_shape_without_raw_keys(self) -> None:
        result = self._run(
            "dev-control", "agent", "choose", "3", "--option", "2", "--fingerprint", "d1.bound.digest"
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.cargo_args.exists())
        self.assertEqual(
            self.bus_args.read_text(encoding="utf-8").splitlines(),
            ["--dev", "agent", "choose", "3", "--option", "2", "--fingerprint", "d1.bound.digest"],
        )
        self.assertNotIn("send-keys", self.bus_args.read_text(encoding="utf-8"))

    def test_dev_control_fails_closed_when_debug_binary_is_missing(self) -> None:
        (self.root / "target" / "debug" / "bus").unlink()
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
