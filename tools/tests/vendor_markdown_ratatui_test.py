from __future__ import annotations

import json
import subprocess
import unittest
from pathlib import Path

PROJECT_ROOT = Path(__file__).resolve().parents[2]
PATCH_DIR = PROJECT_ROOT / "vendor" / "patches" / "markdown-ratatui"
INDEX = PROJECT_ROOT / "vendor" / "markdown-ratatui.patches.md"


class VendorMarkdownRatatuiTests(unittest.TestCase):
    def test_cargo_metadata_resolves_markdown_ratatui_to_vendored_tree(self) -> None:
        result = subprocess.run(
            ["cargo", "metadata", "--locked", "--format-version", "1"],
            cwd=PROJECT_ROOT,
            text=True,
            capture_output=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        packages = [
            package
            for package in json.loads(result.stdout)["packages"]
            if package["name"] == "markdown-ratatui"
        ]
        self.assertEqual(len(packages), 1)
        self.assertEqual(
            Path(packages[0]["manifest_path"]).resolve(),
            (PROJECT_ROOT / "vendor" / "markdown-ratatui" / "Cargo.toml").resolve(),
        )

    def test_every_patch_is_listed_and_applied(self) -> None:
        patches = sorted(PATCH_DIR.glob("*.patch"))
        self.assertTrue(patches)
        index = INDEX.read_text()
        for patch in patches:
            relative = patch.relative_to(PROJECT_ROOT).as_posix()
            self.assertIn(relative, index)
            result = subprocess.run(
                ["git", "apply", "--check", "--reverse", relative],
                cwd=PROJECT_ROOT,
                text=True,
                capture_output=True,
            )
            self.assertEqual(result.returncode, 0, f"{relative} is not applied:\n{result.stderr}")


if __name__ == "__main__":
    unittest.main()
