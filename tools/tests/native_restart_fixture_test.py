"""The restart scenario must distinguish control readiness from client seeding."""
from pathlib import Path
import tempfile
import unittest
from unittest import mock

from tools.acceptance import native_restart_test as acceptance
from tools.acceptance.harness_test import CliError


class AgentSetupReached(Exception):
    pass


class NativeRestartFixtureTests(unittest.TestCase):
    def test_restart_still_sends_before_any_room_or_provider_readiness_poll(self):
        bus = mock.Mock()
        starts = 0

        def start():
            nonlocal starts
            starts += 1
            bus.child.pid = starts

        def cli(*args):
            if args == ("state",):
                self.assertEqual(starts, 1, "restart must send before polling readiness")
                return {"rooms": [{"name": "bus"}], "agents": [
                    {"name": "restart-agent", "runtime_identity": {"pane_id": "pane"}},
                ]}
            if args[:2] == ("agent", "add"):
                return {}
            if args[0] == "send":
                if args[-1] == "immediate-after-restart":
                    self.assertEqual(starts, 2)
                    raise AgentSetupReached
                return {"message_id": 1}
            self.assertEqual(args[0], "wait")
            return {"requests": [{"stage": "replied"}]}

        bus.start.side_effect = start
        bus.cli.side_effect = cli
        bus.native_agent.return_value = {"pane_id": "pane"}
        with tempfile.TemporaryDirectory() as directory, \
                mock.patch.object(acceptance, "Bus", return_value=bus):
            with self.assertRaises(AgentSetupReached):
                acceptance.scenario(Path("/unused/bus"), Path(directory) / "run")
        self.assertEqual(starts, 2)
        bus.wait_native_ready.assert_not_called()

    def test_a_missing_initial_room_fails_with_the_observed_rooms(self):
        bus = mock.Mock()
        bus.cli.return_value = {"rooms": [{"name": "MASTER"}]}
        with mock.patch.object(acceptance.time, "monotonic", side_effect=[0, 31]):
            with self.assertRaisesRegex(AssertionError, "not seeded.*MASTER"):
                acceptance.wait_initial_room(bus)

    def test_control_ready_without_the_default_room_waits_for_client_seeding(self):
        bus = mock.Mock()
        bus.start.return_value = {"rooms": []}
        states = iter([
            {"rooms": [{"name": "MASTER"}]},
            {"rooms": [{"name": "MASTER"}, {"id": 2, "name": "bus"}]},
        ])
        room_ready = False

        def cli(*args):
            nonlocal room_ready
            if args == ("state",):
                state = next(states)
                room_ready = any(room["name"] == "bus" for room in state["rooms"])
                return state
            self.assertEqual(args[:2], ("agent", "add"))
            if not room_ready:
                raise CliError({"message": "No matching room or agent"})
            raise AgentSetupReached

        bus.cli.side_effect = cli
        with tempfile.TemporaryDirectory() as directory, \
                mock.patch.object(acceptance, "Bus", return_value=bus), \
                mock.patch("time.sleep"):
            with self.assertRaises(AgentSetupReached):
                acceptance.scenario(Path("/unused/bus"), Path(directory) / "run")
        self.assertTrue(room_ready)
        bus.stop.assert_called_once()
