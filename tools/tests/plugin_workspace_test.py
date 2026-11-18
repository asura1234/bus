"""Guard the Cargo-level isolation of the vendored plugin.

plugins/feishu-bridge is an independent crate with its own Cargo.toml and
Cargo.lock. It must never become a workspace member or a declared dependency
of the bus package, so Cargo cannot link plugin code into the bus build.
This covers Cargo declarations only; non-Cargo embedding (``include!``,
``#[path]``) is outside its scope.
"""
from __future__ import annotations

import json
import subprocess
import tomllib
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
PLUGIN_PACKAGE = "feishu-bridge"
PLUGIN_DIR = "plugins/feishu-bridge"
DEPENDENCY_TABLES = ("dependencies", "dev-dependencies", "build-dependencies")


def dependency_tables(manifest: dict):
    for name in DEPENDENCY_TABLES:
        yield manifest.get(name, {})
    for target in manifest.get("target", {}).values():
        for name in DEPENDENCY_TABLES:
            yield target.get(name, {})
    yield manifest.get("workspace", {}).get("dependencies", {})
    for patch in manifest.get("patch", {}).values():
        yield patch


class PluginWorkspaceTest(unittest.TestCase):
    def test_cargo_metadata_lists_only_the_bus_package(self) -> None:
        result = subprocess.run(
            ["cargo", "metadata", "--no-deps", "--locked", "--offline", "--format-version", "1"],
            cwd=REPO,
            capture_output=True,
            text=True,
            check=True,
        )
        metadata = json.loads(result.stdout)
        self.assertEqual([package["name"] for package in metadata["packages"]], ["bus"])
        self.assertEqual(len(metadata["workspace_members"]), 1)

    def test_bus_manifest_does_not_depend_on_the_plugin(self) -> None:
        manifest = tomllib.loads((REPO / "Cargo.toml").read_text(encoding="utf-8"))
        for table in dependency_tables(manifest):
            for name, spec in table.items():
                self.assertNotEqual(name, PLUGIN_PACKAGE)
                path = spec.get("path") if isinstance(spec, dict) else None
                if path:
                    normalized = str(path).replace("\\", "/")
                    self.assertNotIn(PLUGIN_DIR, normalized)


if __name__ == "__main__":
    unittest.main()
