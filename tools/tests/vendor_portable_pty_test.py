from __future__ import annotations

import ast
import json
import re
import subprocess
import unittest
from pathlib import Path, PurePosixPath


class VendorPortablePtyTests(unittest.TestCase):
    def test_vendored_tree_contains_required_upstream_files(self) -> None:
        # 测试在 tools/tests/ 下，仓库根是上两级。
        root = Path(__file__).resolve().parents[2] / "vendor" / "portable-pty"
        required = [
            root / "Cargo.toml",
            root / "LICENSE.md",
            root / "src" / "lib.rs",
            root / "src" / "win" / "psuedocon.rs",
        ]

        missing = [str(path.relative_to(root)) for path in required if not path.exists()]
        self.assertEqual(missing, [])

    def test_cargo_patch_points_at_vendored_tree(self) -> None:
        project_root = Path(__file__).resolve().parents[2]
        cargo_toml = (project_root / "Cargo.toml").read_text()

        self.assertIn('portable-pty = "=0.9.0"', cargo_toml)
        self.assertIn("[patch.crates-io]", cargo_toml)
        self.assertIn('portable-pty = { path = "vendor/portable-pty" }', cargo_toml)

    def test_cargo_metadata_resolves_portable_pty_to_vendored_tree(self) -> None:
        project_root = Path(__file__).resolve().parents[2]
        result = subprocess.run(
            ["cargo", "metadata", "--locked", "--format-version", "1"],
            cwd=project_root,
            text=True,
            capture_output=True,
        )
        self.assertEqual(
            result.returncode,
            0,
            f"cargo metadata failed:\nstdout:\n{result.stdout}\nstderr:\n{result.stderr}",
        )

        metadata = json.loads(result.stdout)
        packages = [
            package
            for package in metadata["packages"]
            if package["name"] == "portable-pty" and package["version"] == "0.9.0"
        ]
        self.assertEqual(len(packages), 1)

        manifest_path = Path(packages[0]["manifest_path"]).resolve()
        expected = (project_root / "vendor" / "portable-pty" / "Cargo.toml").resolve()
        self.assertEqual(manifest_path, expected)

    def test_local_vendor_patches_are_listed_in_patch_index(self) -> None:
        project_root = Path(__file__).resolve().parents[2]
        index = project_root / "vendor" / "portable-pty.patches.md"
        patch_dir = project_root / "vendor" / "patches" / "portable-pty"
        patches = sorted(patch_dir.glob("*.patch"))

        if not patches:
            return

        self.assertTrue(index.exists())
        text = index.read_text()
        missing = [
            path.relative_to(project_root).as_posix()
            for path in patches
            if path.relative_to(project_root).as_posix() not in text
        ]
        self.assertEqual(missing, [])

    def test_listed_local_vendor_patches_exist(self) -> None:
        project_root = Path(__file__).resolve().parents[2]
        index = project_root / "vendor" / "portable-pty.patches.md"
        text = index.read_text()
        listed = [
            line.split("`", 2)[1]
            for line in text.splitlines()
            if line.startswith("patch: `vendor/patches/portable-pty/")
        ]

        missing = [path for path in listed if not (project_root / path).exists()]
        self.assertEqual(missing, [])

    def test_local_vendor_patches_are_applied_to_vendored_tree(self) -> None:
        project_root = Path(__file__).resolve().parents[2]
        patch_dir = project_root / "vendor" / "patches" / "portable-pty"

        for patch in sorted(patch_dir.glob("*.patch")):
            result = subprocess.run(
                ["git", "apply", "--check", "--reverse", str(patch.relative_to(project_root))],
                cwd=project_root,
                text=True,
                capture_output=True,
            )
            self.assertEqual(
                result.returncode,
                0,
                f"{patch.relative_to(project_root)} is not applied cleanly:\n"
                f"stdout:\n{result.stdout}\n"
                f"stderr:\n{result.stderr}",
            )

    def test_windows_conpty_loader_uses_only_controlled_sources(self) -> None:
        project_root = Path(__file__).resolve().parents[2]
        source = project_root / "vendor" / "portable-pty" / "src" / "win" / "psuedocon.rs"
        text = source.read_text()

        self.assertIn("std::env::current_exe()", text)
        self.assertIn('.join("conpty")', text)
        self.assertIn('"x64/OpenConsole.exe"', text)
        self.assertIn('"arm64/OpenConsole.exe"', text)
        self.assertIn("LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR", text)
        self.assertIn("LOAD_LIBRARY_SEARCH_SYSTEM32", text)
        self.assertIn("GetModuleHandleW", text)
        self.assertIn("BUS_WINDOWS_CONPTY", text)
        # The package and loader must agree on the marker and allowed file set.
        package_source = project_root / "packaging" / "windows" / "package_conpty.py"
        marker_values = [
            node.value.args[0]
            for node in ast.parse(package_source.read_text()).body
            if isinstance(node, ast.Assign)
            and any(isinstance(target, ast.Name) and target.id == "MARKER_PATH"
                    for target in node.targets)
        ]
        self.assertEqual(len(marker_values), 1)
        package_marker = PurePosixPath(ast.literal_eval(marker_values[0])).name
        loader_marker = re.search(r'let marker = bundle.join\("([^"]+)"\)', text)
        self.assertIsNotNone(loader_marker)
        self.assertEqual(loader_marker.group(1), package_marker)
        self.assertIn(f'BTreeSet::from(["{package_marker}".to_string()])', text)
        self.assertIn("Sha256::new()", text)
        self.assertNotIn('Path::new("conpty.dll")', text)
        self.assertNotIn("shared_library", text)


if __name__ == "__main__":
    unittest.main()
