"""Shared room-assignment consumer branches with a fake verifier; no live Bus process."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


REPO = Path(__file__).resolve().parents[1]
FRAME = "TRUSTED_ROOM_ASSIGNMENT_V1.eyJzY2hlbWEiOiJUUlVTVEVEIn0"


def _load_consumer():
    path = REPO / "cli_extensions" / "room_assignment_context.py"
    spec = importlib.util.spec_from_file_location("room_assignment_context", path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


consumer = _load_consumer()


def verified_stdout(goal: str = "Ship the room", non_goals: str = "No controller") -> str:
    return json.dumps(
        {
            "status": "verified",
            "assignment": {
                "schema": "TRUSTED_ROOM_ASSIGNMENT_V1",
                "request_id": 4,
                "goal": goal,
                "non_goals": non_goals,
            },
        }
    )


class RoomAssignmentContextTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.record = self.root / "calls.jsonl"

    def fake_verifier(self, *, stdout: str = "", exit_code: int = 0, sleep: float = 0.0) -> Path:
        script = self.root / "fake-bus"
        script.write_text(
            "\n".join(
                [
                    f"#!{sys.executable}",
                    "import json, sys, time",
                    f"with open({str(self.record)!r}, 'a', encoding='utf-8') as handle:",
                    "    handle.write(json.dumps(sys.argv[1:]) + '\\n')",
                    f"time.sleep({sleep})",
                    f"sys.stdout.write({stdout!r})",
                    f"sys.exit({exit_code})",
                    "",
                ]
            ),
            encoding="utf-8",
        )
        script.chmod(0o755)
        return script

    def complete_env(self, binary: Path) -> dict[str, str]:
        missing = self.root / "never-created"
        return {
            "BUS_BINARY": str(binary),
            "BUS_TRUSTED_ASSIGNMENT_DIR": str(missing),
            "BUS_TRUSTED_ASSIGNMENT_ENDPOINT": str(missing / "active.json"),
            "BUS_TRUSTED_ASSIGNMENT_TOKEN": "0" * 64,
        }

    def calls(self) -> list[list[str]]:
        if not self.record.exists():
            return []
        return [json.loads(line) for line in self.record.read_text(encoding="utf-8").splitlines()]

    def test_no_frame_and_no_discovery_is_not_in_bus_room_without_verifier_call(self) -> None:
        self.fake_verifier(stdout=verified_stdout())
        context = consumer.load_assignment_context(None, {"PATH": "/usr/bin"})
        self.assertEqual(context, consumer.StandaloneContext())
        self.assertEqual(context.origin, "NotInBusRoom")
        self.assertEqual(self.calls(), [])

    def test_frame_without_discovery_blocks_without_verifier_call(self) -> None:
        self.fake_verifier(stdout=verified_stdout())
        context = consumer.load_assignment_context(FRAME, {})
        self.assertEqual(context, consumer.BlockedContext(reason="incomplete-bus-intent"))
        self.assertEqual(self.calls(), [])

    def test_complete_discovery_without_frame_blocks_without_verifier_call(self) -> None:
        env = self.complete_env(self.fake_verifier(stdout=verified_stdout()))
        for frame in (None, ""):
            with self.subTest(frame=frame):
                context = consumer.load_assignment_context(frame, env)
                self.assertEqual(context, consumer.BlockedContext(reason="incomplete-bus-intent"))
        self.assertEqual(self.calls(), [])

    def test_every_partial_discovery_blocks_without_verifier_call(self) -> None:
        env = self.complete_env(self.fake_verifier(stdout=verified_stdout()))
        for missing in consumer.BUS_DISCOVERY_VARIABLES:
            partial = {name: value for name, value in env.items() if name != missing}
            for frame in (FRAME, None):
                with self.subTest(missing=missing, frame=frame):
                    self.assertIsInstance(
                        consumer.load_assignment_context(frame, partial),
                        consumer.BlockedContext,
                    )
        for only in consumer.BUS_DISCOVERY_VARIABLES:
            with self.subTest(only=only):
                self.assertIsInstance(
                    consumer.load_assignment_context(None, {only: env[only]}),
                    consumer.BlockedContext,
                )
        self.assertEqual(self.calls(), [])

    def test_blank_discovery_value_is_incomplete(self) -> None:
        env = self.complete_env(self.fake_verifier(stdout=verified_stdout()))
        env["BUS_TRUSTED_ASSIGNMENT_TOKEN"] = ""
        self.assertEqual(
            consumer.load_assignment_context(FRAME, env),
            consumer.BlockedContext(reason="incomplete-bus-intent"),
        )
        self.assertEqual(self.calls(), [])

    def test_complete_input_invokes_exact_verifier_and_maps_verified_room_brief(self) -> None:
        env = self.complete_env(
            self.fake_verifier(stdout=verified_stdout("Goal\n- exact", "Non-goal\n- exact"))
        )
        context = consumer.load_assignment_context(FRAME, env)
        self.assertEqual(self.calls(), [["--bus", "assignment", "verify", "--frame", FRAME]])
        self.assertIsInstance(context, consumer.OrchestratedContext)
        self.assertEqual(context.origin, "verified")
        self.assertEqual(context.goal, "Goal\n- exact")
        self.assertEqual(context.non_goals, "Non-goal\n- exact")
        self.assertEqual(context.assignment["request_id"], 4)
        # The consumer never opens assignment storage: discovery paths do not exist.
        self.assertFalse(Path(env["BUS_TRUSTED_ASSIGNMENT_DIR"]).exists())

    def test_invalid_result_blocks(self) -> None:
        env = self.complete_env(
            self.fake_verifier(stdout=json.dumps({"status": "invalid", "reason": "stale"}))
        )
        self.assertEqual(
            consumer.load_assignment_context(FRAME, env),
            consumer.BlockedContext(reason="verifier-invalid:stale"),
        )

    def test_absent_result_is_unreachable_and_blocks(self) -> None:
        env = self.complete_env(self.fake_verifier(stdout=json.dumps({"status": "absent"})))
        self.assertEqual(
            consumer.load_assignment_context(FRAME, env),
            consumer.BlockedContext(reason="verifier-absent-unreachable"),
        )

    def test_malformed_outputs_block(self) -> None:
        cases = {
            "not-json": "verified",
            "list": "[]",
            "missing-assignment": json.dumps({"status": "verified"}),
            "extra-field": json.dumps(
                {"status": "verified", "assignment": {"goal": "g", "non_goals": "n"}, "next": 1}
            ),
            "blank-goal": verified_stdout(goal=" "),
            "blank-non-goals": verified_stdout(non_goals="\n"),
            "invalid-without-reason": json.dumps({"status": "invalid"}),
        }
        for name, stdout in cases.items():
            with self.subTest(case=name):
                env = self.complete_env(self.fake_verifier(stdout=stdout))
                self.assertEqual(
                    consumer.load_assignment_context(FRAME, env),
                    consumer.BlockedContext(reason="verifier-malformed"),
                )

    def test_nonzero_exit_blocks(self) -> None:
        env = self.complete_env(self.fake_verifier(stdout=verified_stdout(), exit_code=3))
        self.assertEqual(
            consumer.load_assignment_context(FRAME, env),
            consumer.BlockedContext(reason="verifier-exit-3"),
        )

    def test_unavailable_executable_blocks(self) -> None:
        env = self.complete_env(self.root / "missing-bus")
        self.assertEqual(
            consumer.load_assignment_context(FRAME, env),
            consumer.BlockedContext(reason="verifier-unavailable"),
        )

    def test_timeout_blocks(self) -> None:
        env = self.complete_env(self.fake_verifier(stdout=verified_stdout(), sleep=5))
        self.assertEqual(
            consumer.load_assignment_context(FRAME, env, timeout=0.3),
            consumer.BlockedContext(reason="verifier-timeout"),
        )

    def test_cli_writes_verified_context_and_removes_stale_output_when_blocked(self) -> None:
        output = self.root / "context" / "assignment.json"
        env = self.complete_env(self.fake_verifier(stdout=verified_stdout()))
        stdout = io.StringIO()
        with mock.patch.dict("os.environ", env, clear=True), contextlib.redirect_stdout(stdout):
            self.assertEqual(consumer.main(["--frame", FRAME, "--output", str(output)]), 0)
        self.assertIn("ASSIGNMENT_ORIGIN=verified", stdout.getvalue())
        written = consumer.read_assignment_context(output)
        self.assertEqual((written.goal, written.non_goals), ("Ship the room", "No controller"))

        stdout = io.StringIO()
        with mock.patch.dict("os.environ", {}, clear=True), contextlib.redirect_stdout(stdout):
            self.assertEqual(consumer.main(["--frame", FRAME, "--output", str(output)]), 1)
        self.assertIn("ASSIGNMENT_ORIGIN=blocked", stdout.getvalue())
        self.assertIn("REASON=incomplete-bus-intent", stdout.getvalue())
        self.assertFalse(output.exists())

        with mock.patch.dict("os.environ", {}, clear=True), contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(consumer.main(["--output", str(output)]), 0)
        self.assertEqual(consumer.read_assignment_context(output), consumer.StandaloneContext())

    def test_read_assignment_context_rejects_non_context_files(self) -> None:
        path = self.root / "context.json"
        for payload in (
            {"origin": "blocked", "reason": "x"},
            {"origin": "verified", "goal": "g", "non_goals": "n"},
            {"origin": "verified", "goal": "", "non_goals": "n", "assignment": {}},
            {"origin": "NotInBusRoom", "goal": "g"},
        ):
            with self.subTest(payload=payload):
                path.write_text(json.dumps(payload), encoding="utf-8")
                with self.assertRaises(ValueError):
                    consumer.read_assignment_context(path)


if __name__ == "__main__":
    unittest.main()
