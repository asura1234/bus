"""Safety boundaries of the no-model Grok integration runner."""
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

from tools.acceptance.grok_fake import Run, poll
from tools.acceptance.harness_test import CliError


class GrokFakeSafetyTest(unittest.TestCase):
    def test_inherited_live_routing_is_rejected_before_opening_a_binary(self):
        for name in ("BUS_DATA_DIR", "BUS_SESSION_ID", "HERDR_SESSION_NAME"):
            with self.subTest(name=name), patch.dict(os.environ, {name: "/live"}, clear=True):
                with self.assertRaisesRegex(ValueError, "scrub"):
                    Run("/does/not/exist")

    def test_binary_must_be_explicit_absolute_and_existing(self):
        with patch.dict(os.environ, {}, clear=True):
            for name in ("target/debug/bus", "/does/not/exist"):
                with self.subTest(name=name), self.assertRaisesRegex(ValueError, "explicit absolute"):
                    Run(name)

    def test_target_debug_binary_is_never_used_for_acceptance(self):
        with tempfile.TemporaryDirectory() as root, patch.dict(os.environ, {}, clear=True):
            binary = Path(root) / "target/debug/bus"
            binary.parent.mkdir(parents=True)
            binary.touch()
            with self.assertRaisesRegex(ValueError, "main checkout"):
                Run(binary)

    def test_driver_reobserves_only_after_a_proven_pre_input_rejection(self):
        run = Run.__new__(Run)
        observation = {"terminal_id": "fixture", "text": "◆ Choose an answer\n1. Alpha\n2. Beta"}
        rejected = CliError({"code": "command_failed", "message": "Terminal is redrawing; observe it again"})
        receipt = {"outcome": "selected", "selection_pending": False, "keys": ["1"]}
        run.cli = Mock(side_effect=[rejected, observation, receipt])
        self.assertEqual(run.driver_action(3, observation, "--key", "1")["outcome"], "selected")
        self.assertEqual([c.args[:2] for c in run.cli.call_args_list],
                         [("agent", "press"), ("agent", "observe"), ("agent", "press")])
        run.cli = Mock(side_effect=[rejected, {**observation, "text": "different menu"}])
        with self.assertRaisesRegex(AssertionError, "changed after rejection"):
            run.driver_action(3, observation, "--key", "1")
        self.assertEqual(run.cli.call_count, 2)

    def test_read_poll_reobserves_only_a_definitely_rejected_native_race(self):
        error = CliError({"code": "command_failed", "message": "Terminal is redrawing; observe it again"})
        read = Mock(side_effect=[error, {"ready": True}])
        with patch("tools.acceptance.grok_fake.time.sleep"):
            self.assertEqual(poll(read, bool), {"ready": True})
        uncertain = CliError({"code": "command_failed", "message": "outcome uncertain"})
        read = Mock(side_effect=uncertain)
        with self.assertRaises(CliError):
            poll(read, bool)
        read.assert_called_once()

    def test_start_keeps_notification_defaults_with_the_private_noop_shim(self):
        run = Run.__new__(Run)
        run.bus, run.cli = Mock(pump_thread=None), Mock()
        with patch("tools.acceptance.grok_fake.fcntl.ioctl"):
            run.start()
        run.bus.start.assert_called_once()
        run.cli.assert_not_called()


class GrokFakeSettlementTest(unittest.TestCase):
    def run_status(self, request):
        run = Run.__new__(Run)
        status = {"complete": True, "requests": [request]}
        run.cli = Mock(return_value=status)
        return run, status

    def test_native_final_ends_without_an_implicit_reply(self):
        run, status = self.run_status({"stage": "ended", "reply": None, "reason": None,
                                       "start_bound": True, "turn_ended": True})
        self.assertEqual(run.settle(1), status)

    def test_explicit_reply_fixture_requires_the_exact_stored_answer(self):
        request = {"stage": "replied", "reply": {"text": "TOKEN"}, "reason": None,
                   "start_bound": True, "turn_ended": False}
        run, status = self.run_status(request)
        self.assertEqual(run.settle(1, reply="TOKEN"), status)
        for change in ({"reply": None}, {"reply": {"text": "wrong"}}, {"stage": "ended"},
                       {"uncertain_outcome": True}, {"reason": "failure"}):
            with self.subTest(change=change):
                run, _ = self.run_status({**request, **change})
                with self.assertRaises(AssertionError):
                    run.settle(1, reply="TOKEN")

    def test_native_completion_rejects_uncertain_failed_or_unended_requests(self):
        request = {"stage": "ended", "reply": None, "reason": None,
                   "start_bound": True, "turn_ended": True}
        for change in ({"reply": {"text": "invented"}}, {"uncertain_outcome": True},
                       {"reason": "failure"}, {"turn_ended": False}, {"start_bound": False}):
            with self.subTest(change=change):
                run, _ = self.run_status({**request, **change})
                with self.assertRaises(AssertionError):
                    run.settle(1)
