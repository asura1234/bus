"""Synthetic trace/mock-recorder tests, never authenticated model calls."""
import copy
import unittest
from unittest.mock import Mock
try:
    from tools.acceptance import grok_probe
except ImportError:
    grok_probe = None


def trace():
    return {
        "authenticated": True, "provenance": "synthetic", "synthetic": True,
        "binary_sha256": "a" * 64, "version": "Grok Build 1.0.5",
        "events": [
            {"kind": "keyboard", "at": 0, "bytes": 500},
            {"kind": "turn_start", "at": 1, "session": "s", "turn": "p1", "source": "keyboard"},
            {"kind": "turn_final", "at": 2, "session": "s", "turn": "p1",
             "text": "GROK_WAITING", "outcome": "success"},
            {"kind": "idle", "at": 3, "session": "s", "turn": "p1"},
            {"kind": "shell_exit", "at": 12, "session": "s", "task": "task-1",
             "text": "GROK_WAKE_DONE", "exit_code": 0, "started_at": 1.2,
             "command": "sleep 8; printf GROK_WAKE_DONE", "background": True, "turn": "p1"},
            {"kind": "turn_start", "at": 13, "session": "s", "turn": "p2",
             "source": "background_task", "task": "task-1"},
            {"kind": "turn_final", "at": 14, "session": "s", "turn": "p2",
             "text": "GROK_WOKE", "outcome": "success"},
        ],
    }


class GrokProbeTests(unittest.TestCase):
    def setUp(self):
        self.assertIsNotNone(grok_probe, "native-wake replay validator is missing")

    def test_grok_unsigned_in_never_invokes_a_recorder(self):
        recorder = Mock(side_effect=AssertionError("model must not start"))
        self.assertEqual(grok_probe.run_probe(recorder=recorder)["status"], "PENDING_AUTH")
        recorder.assert_not_called()

    def test_grok_replay_never_counts_as_native_pass(self):
        self.assertEqual(grok_probe.evaluate_trace(trace())["status"], "REPLAY_VALID")

    def test_grok_native_capture_requires_explicit_live_optin(self):
        with self.assertRaises(ValueError):
            grok_probe.run_probe(authenticated=True, recorder=Mock())

    def test_grok_mock_recorder_cannot_claim_a_native_pass(self):
        result = grok_probe.run_probe(authenticated=True, allow_live_models=True,
                                     recorder=lambda prompt, timeout: trace())
        self.assertEqual(result["status"], "FAIL")
        self.assertEqual(grok_probe.evaluate_trace(trace())["idle_gap_s"], 9)

    def test_grok_fake_or_offline_recorder_cannot_pass(self):
        for field, value in (("provenance", "synthetic"), ("synthetic", True),
                             ("authenticated", False), ("binary_sha256", "")):
            evidence = trace()
            evidence[field] = value
            with self.subTest(field=field):
                result = grok_probe.run_probe(authenticated=True, allow_live_models=True,
                                             recorder=lambda p, t: evidence)
                self.assertNotEqual(result["status"], "PASS")

    def test_grok_notification_without_second_model_turn_is_timeout(self):
        evidence = trace()
        evidence["events"] = evidence["events"][:5]
        evidence["events"].append({"kind": "notification", "at": 13, "text": "GROK_WOKE"})
        self.assertEqual(grok_probe.evaluate_trace(evidence)["status"], "TIMEOUT")

    def test_grok_no_idle_gap_or_polling_in_first_turn_fails(self):
        for replacement in (12, 13):
            evidence = trace()
            evidence["events"][3]["at"] = replacement
            self.assertEqual(grok_probe.evaluate_trace(evidence)["status"], "FAIL")

    def test_grok_extra_keyboard_input_fails_even_before_shell_exit(self):
        evidence = trace()
        evidence["events"].insert(4, {"kind": "keyboard", "at": 4, "bytes": 1})
        self.assertEqual(grok_probe.evaluate_trace(evidence)["status"], "FAIL")

    def test_grok_reused_turn_wrong_session_and_unrelated_wake_fail(self):
        for field, value in (("turn", "p1"), ("session", "other"), ("task", "other"),
                             ("source", "keyboard")):
            evidence = trace()
            evidence["events"][5][field] = value
            with self.subTest(field=field):
                self.assertEqual(grok_probe.evaluate_trace(evidence)["status"], "FAIL")

    def test_grok_wrong_final_cancel_and_failure_are_not_success(self):
        for field, value in (("text", "partial GROK_WOKE"), ("outcome", "cancelled"),
                             ("turn", "p1")):
            evidence = trace()
            evidence["events"][-1][field] = value
            self.assertEqual(grok_probe.evaluate_trace(evidence)["status"], "FAIL")

    def test_grok_invalid_time_and_missing_fields_fail_closed(self):
        for value in (float("nan"), -1, "not-time"):
            evidence = trace()
            evidence["events"][4]["at"] = value
            self.assertEqual(grok_probe.evaluate_trace(evidence)["status"], "FAIL")
        self.assertEqual(grok_probe.evaluate_trace({})["status"], "PENDING_AUTH")

    def test_grok_incomplete_capture_is_timeout(self):
        evidence = trace()
        evidence["events"].pop()
        self.assertEqual(grok_probe.evaluate_trace(evidence)["status"], "TIMEOUT")

    def test_grok_only_successful_owned_sleep_counts(self):
        for field, value in (("exit_code", 1), ("text", "wrong"), ("session", "other")):
            evidence = copy.deepcopy(trace())
            evidence["events"][4][field] = value
            self.assertEqual(grok_probe.evaluate_trace(evidence)["status"], "FAIL")




class GrokMalformedTraceTests(unittest.TestCase):
    def test_grok_malformed_fields_return_failure_instead_of_crashing(self):
        cases = [("binary_sha256", None), ("events", None)]
        for field, value in cases:
            evidence = trace()
            evidence.update(provenance="native-pty", synthetic=False)
            evidence[field] = value
            with self.subTest(field=field):
                self.assertEqual(grok_probe.evaluate_trace(evidence, native=True)["status"], "FAIL")
        for field, value in (("text", None), ("at", -1)):
            evidence = trace()
            evidence["events"][-1 if field == "text" else 0][field] = value
            with self.subTest(field=field):
                self.assertEqual(grok_probe.evaluate_trace(evidence)["status"], "FAIL")


class GrokProbeCliTests(unittest.TestCase):
    def test_grok_cli_is_offline_and_never_labels_synthetic_replay_native(self):
        import json
        import os
        import subprocess
        import sys
        import tempfile
        from pathlib import Path
        env = {k: v for k, v in os.environ.items() if not k.startswith(("BUS_", "HERDR_"))}
        with tempfile.TemporaryDirectory(prefix="grok-f-cli-") as root:
            evidence = Path(root) / "synthetic.json"
            evidence.write_text(json.dumps(trace()))
            command = [sys.executable, "-m", "tools.acceptance.grok_probe"]
            replay = subprocess.run(command + ["--trace", str(evidence)], env=env,
                                    capture_output=True, text=True, timeout=10)
            self.assertEqual(replay.returncode, 0, replay.stderr)
            self.assertEqual(json.loads(replay.stdout)["status"], "REPLAY_VALID")
            pending = subprocess.run(command, env=env, capture_output=True, text=True, timeout=10)
            self.assertEqual(pending.returncode, 2)
            self.assertEqual(json.loads(pending.stdout)["status"], "PENDING_AUTH")




class GrokProbeIdentityTests(unittest.TestCase):
    def test_grok_native_version_matches_captured_build_not_regex_namesake(self):
        for version, expected in (("grok 1.0.5 (5115b46bc909)", "TIMEOUT"),
                                  ("grok 1.0.50", "FAIL"), ("GNU grok 1.0.5", "FAIL")):
            evidence = {"authenticated": True, "provenance": "native-pty",
                        "synthetic": False, "binary_sha256": "a" * 64,
                        "version": version, "events": trace()["events"][:5]}
            with self.subTest(version=version):
                self.assertEqual(grok_probe.evaluate_trace(evidence, native=True)["status"], expected)




class GrokProbeShellTests(unittest.TestCase):
    def test_grok_sentinel_requires_the_owned_background_sleep(self):
        for field, value in (("command", "printf GROK_WAKE_DONE"), ("background", False),
                             ("turn", "wrong"), ("started_at", 11), ("started_at", None)):
            evidence = trace()
            evidence["events"][4][field] = value
            with self.subTest(field=field, value=value):
                self.assertEqual(grok_probe.evaluate_trace(evidence)["status"], "FAIL")




class GrokFakeIdentityTests(unittest.TestCase):
    def test_grok_fake_binary_marker_cannot_be_relabelled_as_native(self):
        evidence = trace()
        evidence.update(provenance="native-pty", synthetic=False,
                        version="grok 1.0.5 (bus-synthetic-fake)")
        self.assertEqual(grok_probe.evaluate_trace(evidence, native=True)["status"], "FAIL")


if __name__ == "__main__":
    unittest.main()
