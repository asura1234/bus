"""The vendored plugin stays outside the bus crate's workspace and binary.

plugins/feishu-bridge is an independent crate with its own Cargo.toml and
Cargo.lock. It must never become a workspace member or a dependency of the
bus package, so no plugin code can be linked into the released bus binary.
"""
from __future__ import annotations

import json
import subprocess
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
PLUGIN_PACKAGE = "feishu-bridge"


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
        manifest = (REPO / "Cargo.toml").read_text(encoding="utf-8")
        self.assertNotIn(PLUGIN_PACKAGE, manifest)


if __name__ == "__main__":
    unittest.main()
