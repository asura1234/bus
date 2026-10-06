import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


sys.path.insert(0, str(Path(__file__).resolve().parent))

from docs_audit import (  # noqa: E402
    build_audit,
    bus_documentation_targets,
    finalize_audit,
    merge_mapper_targets,
    record_target,
    render_pr_section,
    validate_audit,
)


class DocsAuditTest(unittest.TestCase):
    def test_audit_is_fail_closed_until_every_target_is_resolved(self) -> None:
        audit = build_audit(
            base="origin/main",
            merge_base="abc123",
            changed_paths=["shell/packages/example/src/index.ts"],
            target_paths=[
                "shell/packages/example/AGENTS.md",
                "shell/AGENTS.md",
                "AGENTS.md",
            ],
        )

        with self.assertRaisesRegex(ValueError, "pending target"):
            validate_audit(audit, require_complete=True)

        record_target(
            audit,
            path="shell/packages/example/AGENTS.md",
            status="updated",
            reason="Public exports changed.",
        )
        record_target(
            audit,
            path="shell/AGENTS.md",
            status="verified-current",
            reason="Responsibilities and boundaries remain accurate.",
        )
        record_target(
            audit,
            path="AGENTS.md",
            status="verified-current",
            reason="Repository stage summary remains accurate.",
        )
        finalize_audit(audit, verification=["module docs gate: passed"], exclusions=[])
        validate_audit(audit, require_complete=True)

        self.assertEqual(
            render_pr_section(audit),
            "\n".join(
                [
                    "## 文档同步",
                    "",
                    "- [x] `shell/packages/example/AGENTS.md` — updated",
                    "- [x] `shell/AGENTS.md` — verified current",
                    "- [x] `AGENTS.md` — verified current",
                ]
            ),
        )

    def test_audit_rejects_unknown_status_duplicate_targets_and_unsafe_paths(
        self,
    ) -> None:
        with self.assertRaisesRegex(ValueError, "duplicate target"):
            build_audit(
                base="main",
                merge_base="abc123",
                changed_paths=["source.ts"],
                target_paths=["shell/AGENTS.md", "shell/AGENTS.md"],
            )
        with self.assertRaisesRegex(ValueError, "repository-relative"):
            build_audit(
                base="main",
                merge_base="abc123",
                changed_paths=["../outside.ts"],
                target_paths=[],
            )
        # Windows separators must be rejected on the spot rather than treated as a single-segment path:
        # the latter computes depth 0, the "deepest to shallowest" target order silently collapses,
        # and it never reproduces on macOS.
        with self.assertRaisesRegex(ValueError, "POSIX separators"):
            build_audit(
                base="main",
                merge_base="abc123",
                changed_paths=["source.ts"],
                target_paths=["shell\\packages\\example\\AGENTS.md"],
            )
        audit = build_audit(
            base="main",
            merge_base="abc123",
            changed_paths=["source.ts"],
            target_paths=["shell/AGENTS.md"],
        )
        with self.assertRaisesRegex(ValueError, "unknown status"):
            record_target(
                audit,
                path="shell/AGENTS.md",
                status="looks-good",
                reason="not a closed status",
            )

    def test_merge_orders_targets_by_directory_depth(self) -> None:
        """Outputs of multiple mappers are merged, de-duplicated, and reordered by directory depth from deepest to shallowest.

        Deeper first: an umbrella document cites the conclusions of its submodules; the reverse order would check new conclusions against stale ones.
        """
        merged = merge_mapper_targets(
            [
                [
                    "shell/packages/video-editor/core/AGENTS.md",
                    "shell/AGENTS.md",
                    "AGENTS.md",
                ],
                [
                    "shell/packages/video-editor/timeline/AGENTS.md",
                    "shell/AGENTS.md",
                ],
            ]
        )
        self.assertEqual(
            merged,
            [
                "shell/packages/video-editor/core/AGENTS.md",
                "shell/packages/video-editor/timeline/AGENTS.md",
                "shell/AGENTS.md",
                "AGENTS.md",
            ],
        )

    def test_prepare_cli_maps_changed_leaves_to_bus_skill_documentation(self) -> None:
        script = Path(__file__).with_name("docs_audit.py")
        with tempfile.TemporaryDirectory(prefix="docs-audit-") as directory:
            root = Path(directory)
            subprocess.run(["git", "init", "-q", str(root)], check=True)
            subprocess.run(
                ["git", "-C", str(root), "config", "user.email", "test@example.com"],
                check=True,
            )
            subprocess.run(
                ["git", "-C", str(root), "config", "user.name", "Test User"],
                check=True,
            )
            module_root = root / "skills" / "example"
            module_root.mkdir(parents=True)
            (root / "skills" / "AGENTS.md").write_text("# Skills\n", encoding="utf-8")
            (module_root / "SKILL.md").write_text("# Example\n", encoding="utf-8")
            subprocess.run(["git", "-C", str(root), "add", "."], check=True)
            subprocess.run(["git", "-C", str(root), "commit", "-qm", "baseline"], check=True)
            (module_root / "SKILL.md").write_text("# Updated\n", encoding="utf-8")
            output_dir = root / "temp" / "audit"

            # Use sys.executable instead of the literal "python3": Windows usually only has python.exe,
            # the literal fails with FileNotFoundError, and since the failure is raised inside
            # subprocess it reads nothing like "the interpreter name is wrong".
            result = subprocess.run(
                [
                    sys.executable,
                    str(script),
                    "prepare",
                    "--repo",
                    str(root),
                    "--base",
                    "HEAD",
                    "--output-dir",
                    str(output_dir),
                ],
                check=False,
                capture_output=True,
                text=True,
            )
            # No check=True: CalledProcessError carries only the exit code, the child's stderr stays in the
            # exception object and is never printed, so CI shows "exit status 1" with no cause to inspect.
            self.assertEqual(
                result.returncode,
                0,
                f"prepare failed ({result.returncode})\nstdout: {result.stdout}\nstderr: {result.stderr}",
            )

            audit_path = Path(result.stdout.strip())
            audit = json.loads(audit_path.read_text(encoding="utf-8"))
            self.assertEqual(audit["changedLeaves"], ["skills/example/SKILL.md"])
            self.assertEqual(
                [target["path"] for target in audit["targets"]],
                [
                    "skills/AGENTS.md",
                ],
            )

    def test_prepare_all_modules_covers_every_existing_agents_document(self) -> None:
        """The --all-modules target set does not depend on the diff: with no change at all,
        every tracked AGENTS.md (first-party and vendored) must still become a target."""
        script = Path(__file__).with_name("docs_audit.py")
        with tempfile.TemporaryDirectory(prefix="docs-audit-all-") as directory:
            root = Path(directory)
            subprocess.run(["git", "init", "-q", str(root)], check=True)
            subprocess.run(
                ["git", "-C", str(root), "config", "user.email", "test@example.com"],
                check=True,
            )
            subprocess.run(["git", "-C", str(root), "config", "user.name", "Test User"], check=True)
            (root / "skills").mkdir()
            (root / "skills" / "AGENTS.md").write_text("# Skills\n", encoding="utf-8")
            vendored = root / "vendor" / "example" / "src"
            vendored.mkdir(parents=True)
            (root / "vendor" / "example" / "AGENTS.md").write_text("# Vendor\n", encoding="utf-8")
            (vendored / "AGENTS.md").write_text("# Vendor src\n", encoding="utf-8")
            subprocess.run(["git", "-C", str(root), "add", "."], check=True)
            subprocess.run(["git", "-C", str(root), "commit", "-qm", "baseline"], check=True)

            result = subprocess.run(
                [
                    sys.executable,
                    str(script),
                    "prepare",
                    "--repo",
                    str(root),
                    "--base",
                    "HEAD",
                    "--all-modules",
                    "--output-dir",
                    str(root / "temp" / "audit"),
                ],
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(
                result.returncode,
                0,
                f"prepare failed ({result.returncode})\nstdout: {result.stdout}\nstderr: {result.stderr}",
            )
            audit = json.loads(Path(result.stdout.strip()).read_text(encoding="utf-8"))
            self.assertEqual(audit["changedLeaves"], [])
            self.assertEqual(
                [target["path"] for target in audit["targets"]],
                [
                    "vendor/example/src/AGENTS.md",
                    "vendor/example/AGENTS.md",
                    "skills/AGENTS.md",
                ],
            )

    def test_bus_mapper_uses_nearest_existing_vendor_agents_files(self) -> None:
        repo = Path(__file__).resolve().parents[3]
        self.assertEqual(
            bus_documentation_targets(
                repo,
                ["vendor/libghostty-vt/src/terminal/c/terminal.zig"],
            ),
            [
                "vendor/libghostty-vt/src/terminal/c/AGENTS.md",
                "vendor/libghostty-vt/AGENTS.md",
            ],
        )


if __name__ == "__main__":
    unittest.main()
