import base64
import contextlib
import io
import shlex
import subprocess
import sys
import tempfile
import unittest
from concurrent.futures import Future
from pathlib import Path
from unittest.mock import patch


sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from gate_and_fix import (  # noqa: E402
    Gate,
    GateResult,
    main,
    render_round,
    run_gates,
    select_gates,
    validate_round,
)


_CI_TOOLS = frozenset({"just", "cargo-nextest"})


class GateAndFixTest(unittest.TestCase):
    def test_every_diff_always_runs_four_checks_and_diff(self) -> None:
        for changed in (
            ["src/main.rs"],
            ["skills/pr/SKILL.md"],
            ["docs/guides/review-format.md"],
        ):
            for tools in (_CI_TOOLS, frozenset()):
                with self.subTest(changed=changed, tools=tools):
                    gates = select_gates(
                        changed, base="baseline-sha", available_tools=tools
                    )
                    self.assertEqual(
                        [g.name for g in gates],
                        ["lint", "unit", "integration", "coverage", "diff-check"],
                    )
                    self.assertTrue(
                        all(g.requires_exclusive_execution for g in gates[:4])
                    )
                    self.assertEqual(
                        gates[-1].argv,
                        ("git", "diff", "--check", "baseline-sha...HEAD"),
                    )

    def test_fallback_and_just_use_the_same_check_implementation(self) -> None:
        repo = Path(__file__).resolve().parents[4]
        text = (repo / "justfile").read_text(encoding="utf-8")
        direct = select_gates(["src/main.rs"], base="base", available_tools=frozenset())
        wrapped = select_gates(["src/main.rs"], base="base", available_tools=_CI_TOOLS)
        for fallback, wrapper in zip(direct[:4], wrapped[:4]):
            recipe = text.split("\n" + wrapper.argv[-1] + ":\n", 1)[1].split("\n\n", 1)[
                0
            ]
            command = tuple(
                shlex.split(recipe.strip().replace("{{python}}", sys.executable))
            )
            self.assertEqual(fallback.argv, command)
            self.assertEqual(
                wrapper.argv[:4], ("just", "--set", "python", sys.executable)
            )

    def test_tool_preflight_requires_both_ci_tools(self) -> None:
        for available in (
            frozenset(),
            frozenset({"just"}),
            frozenset({"cargo-nextest"}),
            _CI_TOOLS,
        ):
            with self.subTest(available=sorted(available)):
                with patch(
                    "gate_and_fix.shutil.which",
                    side_effect=lambda name: (
                        f"/tools/{name}" if name in available else None
                    ),
                ):
                    gates = select_gates(["src/main.rs"], base="base")
                self.assertEqual(
                    gates[0].argv[0],
                    "just" if available == _CI_TOOLS else sys.executable,
                )
                self.assertEqual(len(gates), 5)

    def test_select_gates_rejects_non_repository_paths(self) -> None:
        with self.assertRaisesRegex(ValueError, "repository-relative"):
            select_gates(["../outside.py"], base="base")

    def test_select_gates_never_runs_build_or_live_e2e(self) -> None:
        for tools in (_CI_TOOLS, frozenset()):
            for gate in select_gates(
                ["src/main.rs"], base="base", available_tools=tools
            ):
                self.assertNotIn("build", gate.argv)
                self.assertNotIn("e2e", gate.argv)

    def test_run_gates_starts_independent_gates_before_either_can_finish(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            child = (
                "from pathlib import Path\n"
                "import sys, time\n"
                "root = Path(sys.argv[1])\n"
                "mine = root / sys.argv[2]\n"
                "other = root / sys.argv[3]\n"
                "mine.touch()\n"
                "deadline = time.monotonic() + 2\n"
                "while not other.exists():\n"
                "    if time.monotonic() >= deadline:\n"
                "        raise SystemExit(9)\n"
                "    time.sleep(0.01)\n"
            )
            results = run_gates(
                [
                    Gate("first", (sys.executable, "-c", child, str(root), "first", "second")),
                    Gate("second", (sys.executable, "-c", child, str(root), "second", "first")),
                ],
                cwd=Path.cwd(),
            )

        self.assertEqual([result.exit_code for result in results], [0, 0])

    def test_run_gates_does_not_overlap_a_shared_writable_resource(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            child = (
                "from pathlib import Path\n"
                "import sys, time\n"
                "active = Path(sys.argv[1]) / 'active'\n"
                "if active.exists():\n"
                "    raise SystemExit(9)\n"
                "active.touch()\n"
                "try:\n"
                "    time.sleep(0.1)\n"
                "finally:\n"
                "    active.unlink()\n"
            )
            gates = [
                Gate(
                    "native-lint",
                    (sys.executable, "-c", child, str(root)),
                    frozenset({"native-build"}),
                ),
                Gate(
                    "native-coverage",
                    (sys.executable, "-c", child, str(root)),
                    frozenset({"native-build"}),
                ),
            ]
            results = run_gates(gates, cwd=Path.cwd())

        self.assertEqual([result.exit_code for result in results], [0, 0])

    def test_run_gates_isolates_measurement_before_between_and_after_ordinary_gates(self) -> None:
        for position in range(3):
            with self.subTest(position=position):
                gates = [Gate("unit", ("unit",)), Gate("coverage", ("coverage",))]
                gates.insert(
                    position,
                    Gate("measurement", ("measure",), requires_exclusive_execution=True),
                )
                submitted: dict[Future, Gate] = {}
                batches: list[set[str]] = []

                def submit(_run, gate, *, cwd):
                    future = Future()
                    submitted[future] = gate
                    return future

                def complete_one(active, *, return_when):
                    # Control completion order here so the admission assertions do not depend on whether a real
                    # child process happens to finish before the next one starts.
                    batch = {submitted[future].name for future in active}
                    batches.append(batch)
                    if "measurement" in batch:
                        self.assertEqual(batch, {"measurement"})
                    future = next(iter(active))
                    gate = submitted[future]
                    future.set_result(GateResult(gate.name, gate.argv, 0, 0, "", ""))
                    return {future}, set(active) - {future}

                with (
                    patch("gate_and_fix.ThreadPoolExecutor") as executor,
                    patch("gate_and_fix.wait", side_effect=complete_one),
                ):
                    executor.return_value.__enter__.return_value.submit.side_effect = submit
                    results = run_gates(gates, cwd=Path.cwd())

                self.assertIn({"unit", "coverage"}, batches)
                self.assertEqual([result.name for result in results], [gate.name for gate in gates])
                self.assertTrue(all(result.passed for result in results))

    def test_run_gates_rejects_duplicate_names_before_launching_a_gate(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            marker = Path(directory) / "launched"
            gate = Gate(
                "duplicate",
                (
                    sys.executable,
                    "-c",
                    "from pathlib import Path; Path(__import__('sys').argv[1]).touch()",
                    str(marker),
                ),
            )

            with self.assertRaisesRegex(ValueError, "duplicate gate name"):
                run_gates([gate, gate], cwd=Path.cwd())

            self.assertFalse(marker.exists())

    def test_render_round_round_trips_complete_output_and_validates_it(self) -> None:
        samples = ("", "no final newline", "final newline\n", "trailing whitespace \t\n")
        for sample in samples:
            with self.subTest(sample=repr(sample)):
                artifact = render_round(
                    round_number=2,
                    base="base-sha",
                    head="head-sha",
                    changed_files=["src/main.rs"],
                    results=[
                        GateResult(
                            name="failure",
                            argv=("tool", "--arg"),
                            exit_code=1,
                            duration_ms=2,
                            stdout=sample,
                            stderr=sample,
                        )
                    ],
                )

                self.assertEqual(self._decode_rendered_log(artifact, "stdout"), sample)
                self.assertEqual(self._decode_rendered_log(artifact, "stderr"), sample)
                self.assertEqual(validate_round(artifact, expected_base="base-sha"), "FAIL")

    def test_render_round_accepts_a_negative_exit_code_as_a_failure(self) -> None:
        artifact = render_round(
            round_number=1,
            base="base-sha",
            head="head-sha",
            changed_files=["src/main.rs"],
            results=[
                GateResult(
                    name="interrupted",
                    argv=("tool",),
                    exit_code=-9,
                    duration_ms=1,
                    stdout="",
                    stderr="",
                )
            ],
        )

        self.assertIn("### interrupted — FAIL", artifact)
        self.assertIn("- Exit code: `-9`", artifact)
        self.assertEqual(validate_round(artifact, expected_base="base-sha"), "FAIL")

    def test_run_gates_records_a_launch_error_that_renders_and_validates(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            missing = Path(directory) / "does-not-exist"
            result = run_gates([Gate("missing", (str(missing),))], cwd=Path.cwd())[0]

        self.assertIsNone(result.exit_code)
        self.assertTrue(result.stderr.startswith("launch error: "))
        artifact = render_round(
            round_number=1,
            base="base-sha",
            head="head-sha",
            changed_files=["skills/gate-and-fix/SKILL.md"],
            results=[result],
        )
        self.assertIn("### missing — FAIL", artifact)
        self.assertIn("- Exit code: `launch-error`", artifact)
        self.assertEqual(validate_round(artifact, expected_base="base-sha"), "FAIL")

    def test_main_writes_a_fresh_artifact_for_each_invocation(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory)
            (repo / ".gitignore").write_text("artifacts/\n", encoding="utf-8")
            source = repo / "skills/gate-and-fix/scripts/example.py"
            source.parent.mkdir(parents=True)
            source.write_text("before\n", encoding="utf-8")
            self._git(repo, "init", "-q")
            self._git(repo, "config", "user.email", "test@example.com")
            self._git(repo, "config", "user.name", "Test User")
            self._git(repo, "add", ".")
            self._git(repo, "commit", "-qm", "initial")
            base = self._git_output(repo, "rev-parse", "HEAD")
            source.write_text("after\n", encoding="utf-8")
            self._git(repo, "add", "skills/gate-and-fix/scripts/example.py")
            self._git(repo, "commit", "-qm", "change")

            artifact_root = repo / "artifacts"
            # The real gates would run `just ci` against this scratch repository; the artifact
            # lifecycle under test does not depend on what the gates execute.
            passing = [
                GateResult("ci", ("just", "ci"), 0, 1, "", ""),
                GateResult("diff-check", ("git", "diff", "--check", f"{base}...HEAD"), 0, 1, "", ""),
            ]
            with patch("gate_and_fix.run_gates", return_value=passing):
                first = self._run_main(repo, base, artifact_root)
                second = self._run_main(repo, base, artifact_root)

            self.assertNotEqual(first, second)
            self.assertTrue(first.is_file())
            self.assertTrue(second.is_file())
            self.assertEqual(validate_round(first.read_text(encoding="utf-8"), expected_base=base), "PASS")
            self.assertEqual(validate_round(second.read_text(encoding="utf-8"), expected_base=base), "PASS")

    def test_main_rejects_a_dirty_worktree_without_an_artifact(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory)
            (repo / ".gitignore").write_text("artifacts/\n", encoding="utf-8")
            source = repo / "skills/gate-and-fix/scripts/example.py"
            source.parent.mkdir(parents=True)
            source.write_text("before\n", encoding="utf-8")
            self._git(repo, "init", "-q")
            self._git(repo, "config", "user.email", "test@example.com")
            self._git(repo, "config", "user.name", "Test User")
            self._git(repo, "add", ".")
            self._git(repo, "commit", "-qm", "initial")
            base = self._git_output(repo, "rev-parse", "HEAD")
            source.write_text("after\n", encoding="utf-8")
            self._git(repo, "add", "skills/gate-and-fix/scripts/example.py")
            self._git(repo, "commit", "-qm", "change")
            source.write_text("dirty tracked change\n", encoding="utf-8")
            (repo / "untracked.txt").write_text("dirty\n", encoding="utf-8")

            stdout = io.StringIO()
            stderr = io.StringIO()
            artifact_root = repo / "artifacts"
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                exit_code = main(
                    [
                        "--repo",
                        str(repo),
                        "--base",
                        base,
                        "--round",
                        "1",
                        "--artifact-root",
                        str(artifact_root),
                    ]
                )

            self.assertEqual(exit_code, 2)
            self.assertEqual(stdout.getvalue(), "")
            self.assertIn("worktree", stderr.getvalue())
            self.assertFalse(artifact_root.exists())

    def test_main_reports_runner_errors_without_an_artifact(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            stdout = io.StringIO()
            stderr = io.StringIO()
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                exit_code = main(
                    [
                        "--repo",
                        directory,
                        "--base",
                        "missing",
                        "--round",
                        "1",
                        "--artifact-root",
                        f"{directory}/artifacts",
                    ]
                )

        self.assertEqual(exit_code, 2)
        self.assertEqual(stdout.getvalue(), "")
        self.assertIn("gate-and-fix runner failed", stderr.getvalue())

    def test_verify_reports_the_validated_outcome(self) -> None:
        artifact = render_round(
            round_number=1,
            base="base-sha",
            head="head-sha",
            changed_files=["skills/gate-and-fix/SKILL.md"],
            results=[
                GateResult(
                    name="ci",
                    argv=("just", "ci"),
                    exit_code=0,
                    duration_ms=1,
                    stdout="",
                    stderr="",
                )
            ],
        )
        with tempfile.TemporaryDirectory() as directory:
            artifact_path = Path(directory) / "round.md"
            artifact_path.write_text(artifact, encoding="utf-8")
            stdout = io.StringIO()
            with contextlib.redirect_stdout(stdout):
                self.assertEqual(
                    main(
                        [
                            "verify",
                            "--artifact",
                            str(artifact_path),
                            "--base",
                            "base-sha",
                        ]
                    ),
                    0,
                )

        self.assertEqual(stdout.getvalue(), "PASS\n")

    def test_show_outputs_one_validated_log_stream_without_a_newline(self) -> None:
        artifact = render_round(
            round_number=1,
            base="base-sha",
            head="head-sha",
            changed_files=["skills/gate-and-fix/SKILL.md"],
            results=[
                GateResult(
                    name="lint",
                    argv=("cargo", "clippy"),
                    exit_code=1,
                    duration_ms=1,
                    stdout="first line\nlast line",
                    stderr="error\n",
                )
            ],
        )
        with tempfile.TemporaryDirectory() as directory:
            artifact_path = Path(directory) / "round.md"
            artifact_path.write_text(artifact, encoding="utf-8")
            stdout = io.StringIO()
            with contextlib.redirect_stdout(stdout):
                self.assertEqual(
                    main(
                        [
                            "show",
                            "--artifact",
                            str(artifact_path),
                            "--base",
                            "base-sha",
                            "--gate",
                            "lint",
                            "--stream",
                            "stdout",
                        ]
                    ),
                    0,
                )

        self.assertEqual(stdout.getvalue(), "first line\nlast line")

    def test_list_outputs_only_failed_gate_names(self) -> None:
        artifact = render_round(
            round_number=1,
            base="base-sha",
            head="head-sha",
            changed_files=["skills/gate-and-fix/SKILL.md"],
            results=[
                GateResult(
                    name="format",
                    argv=("cargo", "fmt", "--check"),
                    exit_code=0,
                    duration_ms=1,
                    stdout="",
                    stderr="",
                ),
                GateResult(
                    name="lint",
                    argv=("cargo", "clippy"),
                    exit_code=1,
                    duration_ms=1,
                    stdout="failure",
                    stderr="",
                ),
            ],
        )
        with tempfile.TemporaryDirectory() as directory:
            artifact_path = Path(directory) / "round.md"
            artifact_path.write_text(artifact, encoding="utf-8")
            stdout = io.StringIO()
            with contextlib.redirect_stdout(stdout):
                self.assertEqual(
                    main(
                        [
                            "list",
                            "--artifact",
                            str(artifact_path),
                            "--base",
                            "base-sha",
                        ]
                    ),
                    0,
                )

        self.assertEqual(stdout.getvalue(), "lint\n")

    def test_show_rejects_an_artifact_with_the_wrong_base(self) -> None:
        artifact = render_round(
            round_number=1,
            base="base-sha",
            head="head-sha",
            changed_files=["skills/gate-and-fix/SKILL.md"],
            results=[
                GateResult(
                    name="lint",
                    argv=("cargo", "clippy"),
                    exit_code=1,
                    duration_ms=1,
                    stdout="failure",
                    stderr="",
                )
            ],
        )
        with tempfile.TemporaryDirectory() as directory:
            artifact_path = Path(directory) / "round.md"
            artifact_path.write_text(artifact, encoding="utf-8")
            stdout = io.StringIO()
            stderr = io.StringIO()
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                self.assertEqual(
                    main(
                        [
                            "show",
                            "--artifact",
                            str(artifact_path),
                            "--base",
                            "other-base",
                            "--gate",
                            "lint",
                            "--stream",
                            "stdout",
                        ]
                    ),
                    2,
                )

        self.assertEqual(stdout.getvalue(), "")
        self.assertIn("Base", stderr.getvalue())

    @staticmethod
    def _git(repo: Path, *arguments: str) -> None:
        subprocess.run(("git", *arguments), cwd=repo, check=True)

    @staticmethod
    def _git_output(repo: Path, *arguments: str) -> str:
        return subprocess.run(
            ("git", *arguments),
            cwd=repo,
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()

    def _run_main(self, repo: Path, base: str, artifact_root: Path) -> Path:
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            self.assertEqual(
                main(
                    [
                        "--repo",
                        str(repo),
                        "--base",
                        base,
                        "--round",
                        "1",
                        "--artifact-root",
                        str(artifact_root),
                    ]
                ),
                0,
            )
        return Path(stdout.getvalue().strip())

    @staticmethod
    def _decode_rendered_log(artifact: str, title: str) -> str:
        lines = artifact.splitlines()
        index = lines.index(f"#### {title}")
        assert lines[index + 1] == ""
        assert lines[index + 2] == "- Encoding: `base64-utf8`"
        byte_count = int(lines[index + 3].removeprefix("- Bytes: `").removesuffix("`"))
        assert lines[index + 4] == ""
        assert lines[index + 5] == "```base64"
        raw = base64.b64decode(lines[index + 6], validate=True)
        assert len(raw) == byte_count
        assert lines[index + 7] == "```"
        return raw.decode("utf-8")


if __name__ == "__main__":
    unittest.main()
