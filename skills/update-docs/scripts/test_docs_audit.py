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
        # Windows 分隔符必须当场拒绝而不是被当成单段路径：后者会让深度算成 0，
        # "从深到浅" 的目标顺序静默塌掉，且在 macOS 上永远复现不出来。
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
        """多个 mapper 的输出合并去重后按目录深度从深到浅重排。

        深的先做：伞级文档要引用子模块的结论，反过来做会拿旧结论去校对新的。
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
            subprocess.run(
                ["git", "-C", str(root), "commit", "-qm", "baseline"], check=True
            )
            (module_root / "SKILL.md").write_text("# Updated\n", encoding="utf-8")
            output_dir = root / "temp" / "audit"

            # 用 sys.executable 而不是字面量 "python3"：Windows 上通常只有 python.exe，
            # 字面量会以 FileNotFoundError 失败，而失败点在 subprocess 内部，读起来
            # 完全不像「解释器名字写错了」。
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
            # 不用 check=True：CalledProcessError 只带退出码，子进程的 stderr 留在异常
            # 对象里从不打印，CI 上的表现是「exit status 1」而没有任何原因可查。
            self.assertEqual(
                result.returncode,
                0,
                f"prepare failed ({result.returncode})\n"
                f"stdout: {result.stdout}\nstderr: {result.stderr}",
            )

            audit_path = Path(result.stdout.strip())
            audit = json.loads(audit_path.read_text(encoding="utf-8"))
            self.assertEqual(
                audit["changedLeaves"], ["skills/example/SKILL.md"]
            )
            self.assertEqual(
                [target["path"] for target in audit["targets"]],
                [
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
