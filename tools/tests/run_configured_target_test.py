from __future__ import annotations

import json
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
REAL_CARGO = shutil.which("cargo")


@unittest.skipUnless(os.name == "posix" and REAL_CARGO, "requires POSIX and Cargo")
class RunConfiguredTargetTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory(prefix="bus-run-config-", dir="/private/tmp")
        self.addCleanup(self.temp_dir.cleanup)
        self.root = Path(self.temp_dir.name).resolve()
        self.bin_dir = self.root / "bin"
        self.bin_dir.mkdir()
        self.bus_args = self.root / "bus-args"
        self.env = {
            key: value
            for key, value in os.environ.items()
            if key not in ("CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR", "CARGO_BUILD_TARGET")
        }
        self.env["BUS_DATA_DIR"] = str(self.root / "data")
        shutil.copy2(REPO_ROOT / "run", self.root / "run")
        (self.root / "run").chmod(0o755)

        # A real crate named bus, built by real Cargo, so the launcher is
        # checked against Cargo's own build.target-dir handling.
        (self.root / "src").mkdir()
        (self.root / "src" / "main.rs").write_text(
            "use std::io::Write;\n"
            "fn main() {\n"
            '    let mut f = std::fs::File::create(std::env::var("FAKE_BUS_ARGS").unwrap()).unwrap();\n'
            '    writeln!(f, "fresh-configured").unwrap();\n'
            '    for arg in std::env::args().skip(1) { writeln!(f, "{arg}").unwrap(); }\n'
            "}\n",
            encoding="utf-8",
        )
        (self.root / "Cargo.toml").write_text(
            '[package]\nname = "bus"\nversion = "0.0.0"\nedition = "2021"\n'
            '[features]\ndev-tools = []\n',
            encoding="utf-8",
        )
        (self.root / ".cargo").mkdir()
        (self.root / ".cargo" / "config.toml").write_text(
            '[build]\ntarget-dir = "configured-build"\n', encoding="utf-8"
        )
        for argv in (
            [REAL_CARGO, "generate-lockfile", "--offline"],
            [REAL_CARGO, "metadata", "--offline", "--no-deps", "--format-version", "1"],
        ):
            done = subprocess.run(
                argv, cwd=self.root, env=self.env, capture_output=True, text=True, check=True
            )
        self.target_dir = Path(json.loads(done.stdout)["target_directory"])
        self.assertEqual(self.target_dir, self.root / "configured-build")
        self.env["FAKE_BUS_ARGS"] = str(self.bus_args)
        # A stale binary at the default location must never be the one launched.
        for profile in ("debug", "release"):
            self._write_executable(
                self.root / "target" / profile / "bus",
                """#!/bin/sh
{ echo default-target; printf '%s\\n' "$@"; } > "$FAKE_BUS_ARGS"
""",
            )

    @staticmethod
    def _write_executable(path: Path, content: str) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
        path.chmod(0o755)

    def test_prod_launches_the_binary_from_cargos_configured_target_directory(self) -> None:
        self._assert_configured_launch("prod", ["fresh-configured", "--paths"])

    def test_dev_launches_the_binary_from_cargos_configured_target_directory(self) -> None:
        self._assert_configured_launch("dev", ["fresh-configured", "--dev", "--paths"])

    def _assert_configured_launch(self, mode: str, expected: list[str]) -> None:
        result = subprocess.run(
            [str(self.root / "run"), mode, "--paths"],
            cwd=self.root.parent,
            env=self.env,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.bus_args.read_text(encoding="utf-8").splitlines(), expected)

    def test_dev_control_keeps_the_default_debug_binary_with_a_target_override(self) -> None:
        result = subprocess.run(
            [str(self.root / "run"), "dev-control", "state"],
            cwd=self.root.parent,
            env={**self.env, "CARGO_TARGET_DIR": str(self.target_dir)},
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            self.bus_args.read_text(encoding="utf-8").splitlines(),
            ["default-target", "--dev", "state"],
        )
        self.assertFalse(self.target_dir.exists(), "dev-control never builds")
