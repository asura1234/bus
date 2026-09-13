"""Acceptance-driver profiles and result validation; no live model calls."""
import contextlib
import io
import unittest
from unittest import mock

from scripts import bus_dev_acceptance as acceptance


class AcceptanceProfileTests(unittest.TestCase):
    def test_six_agent_profile_covers_every_agent_and_group_sizes_in_twenty_messages(self):
        self.assertIn("six-agents", acceptance.profiles())
        profile = acceptance.profiles()["six-agents"]
        self.assertEqual(profile["providers"], ("claude", "codex", "cursor", "claude", "codex", "cursor"))
        self.assertEqual(profile["messages"], 20)
        self.assertEqual(len(profile["recipients"]), 20)
        self.assertEqual({group[0] for group in profile["recipients"] if len(group) == 1}, set(range(6)))
        self.assertEqual({len(group) for group in profile["recipients"]}, {1, 2, 3, 6})
        self.assertEqual(profile["recipients"][0], (2,))
        args = acceptance.parse_args(["--allow-live-models", "--profile", "six-agents",
                                      "--claude-pwd", str(acceptance.ROOT), "--codex-pwd", str(acceptance.ROOT),
                                      "--cursor-pwd", str(acceptance.ROOT)])
        self.assertEqual(args.messages, 20)

    def test_default_user_root_requires_explicit_opt_in_and_nonempty_path(self):
        self.assertTrue(hasattr(acceptance, "validate_data_root"))
        user_root = acceptance.Path.home() / ".local/share/bus"
        with self.assertRaises(SystemExit):
            acceptance.validate_data_root(str(user_root), False)
        with self.assertRaises(SystemExit):
            acceptance.validate_data_root("", True)
        self.assertEqual(acceptance.validate_data_root(str(user_root), True), user_root.resolve())
        self.assertEqual(acceptance.validate_data_root(str(acceptance.ROOT / "temp/probe"), False),
                         acceptance.ROOT / "temp/probe")

    def test_three_provider_profile_runs_each_provider_alone_pairs_and_all(self):
        profile = acceptance.profiles()["claude-codex-cursor"]
        self.assertEqual(profile["providers"], ("claude", "codex", "cursor"))
        self.assertEqual(profile["messages"], 15)
        self.assertEqual(profile["recipients"], ((0,), (1,), (2,), (0, 1), (0, 2), (1, 2), (0, 1, 2)))

    def test_original_four_agent_profile_is_preserved(self):
        profile = acceptance.profiles()["claude-codex"]
        self.assertEqual(profile["providers"], ("claude", "codex", "claude", "codex"))
        self.assertEqual(profile["messages"], 20)
        self.assertEqual(profile["recipients"], ((0,), (1,), (0, 1), (0, 1, 2), (0, 1, 2, 3)))

    def test_cursor_profile_requires_pwd_before_any_live_setup(self):
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            acceptance.parse_args(["--allow-live-models", "--profile", "claude-codex-cursor",
                                   "--claude-pwd", "/tmp/claude", "--codex-pwd", "/tmp/codex"])
        args = acceptance.parse_args(["--allow-live-models", "--profile", "claude-codex-cursor",
                                      "--claude-pwd", "/tmp/claude", "--codex-pwd", "/tmp/codex",
                                      "--cursor-pwd", "/tmp/cursor"])
        self.assertEqual(args.messages, 15)
        self.assertEqual(args.cursor_pwd, "/tmp/cursor")

    def test_delivery_verifier_rejects_wrong_target_and_uncorrelated_cursor_reply(self):
        response = {"complete": True, "requests": [{"agent_id": 3, "stage": "replied",
                    "start_bound": True, "session_id": "cursor-conversation", "reply": {"text": "TOKEN"}}]}
        acceptance.verify_delivery(response, [3], "TOKEN")
        with self.assertRaises(AssertionError):
            acceptance.verify_delivery(response, [2], "TOKEN")
        response["requests"][0]["start_bound"] = False
        with self.assertRaises(AssertionError):
            acceptance.verify_delivery(response, [3], "TOKEN")


class AcceptanceObservationTests(unittest.TestCase):
    def test_all_selector_requires_exact_owned_room_membership(self):
        self.assertTrue(hasattr(acceptance, "recipient_selector"))
        agents = [{"id": 3}, {"id": 4}]
        def state(*argv):
            self.assertEqual(argv, ("state",))
            return {"agents": [{"id": 3, "room_id": 2}, {"id": 4, "room_id": 2},
                               {"id": 99, "room_id": 7}]}
        self.assertEqual(acceptance.recipient_selector(state, 2, agents, agents), "all")
        self.assertEqual(acceptance.recipient_selector(None, 2, agents, [agents[0]]), "3")
        for members in ([3, 4, 5], [3]):
            with self.assertRaises(AssertionError):
                acceptance.recipient_selector(lambda *_: {"agents": [{"id": i, "room_id": 2}
                                                                       for i in members]}, 2, agents, agents)

    def test_setup_preflight_rejects_sent_requests_and_any_terminal_leak(self):
        self.assertTrue(hasattr(acceptance, "verify_setup_gate"))
        queued = {"complete": False, "requests": [{"agent_id": 3, "request_id": 9,
                  "stage": "queued", "reason": "hook_setup_unconfirmed", "start_bound": False,
                  "session_id": None, "uncertain_outcome": False, "reply": None}]}
        acceptance.verify_setup_gate(queued, [3], "TOKEN", {"cursor1": "idle", "claude1": "idle"})
        for patch in ({"stage": "delivered"}, {"reason": "agent_working"}, {"reply": {"text": "TOKEN"}}):
            changed = {**queued, "requests": [{**queued["requests"][0], **patch}]}
            with self.assertRaises(AssertionError):
                acceptance.verify_setup_gate(changed, [3], "TOKEN", {"cursor1": "idle"})
        with self.assertRaises(AssertionError):
            acceptance.verify_setup_gate(queued, [4], "TOKEN", {"cursor1": "idle"})
        with self.assertRaises(AssertionError):
            acceptance.verify_setup_gate(queued, [3], "TOKEN", {"cursor1": "idle", "claude1": "TOKEN"})

    def test_observed_status_coverage_cannot_be_satisfied_by_other_agents(self):
        self.assertTrue(hasattr(acceptance, "observe_statuses"))
        self.assertTrue(hasattr(acceptance, "verify_status_coverage"))
        agents = [{"id": 3, "provider": "cursor"}, {"id": 4, "provider": "claude"}]
        samples = []
        acceptance.observe_statuses(samples, {"agents": [{"agent_id": 99, "status": "working"},
                                      {"agent_id": 3, "status": "idle"},
                                      {"agent_id": 4, "status": "working"}]}, agents, [3], 0)
        self.assertEqual([(s["agent_id"], s["status"]) for s in samples], [(3, "idle")])
        with self.assertRaises(AssertionError):
            acceptance.verify_status_coverage([{"status_samples": samples}], agents)
        for state in ("working", "idle"):
            acceptance.observe_statuses(samples, {"agents": [{"agent_id": 3, "status": state},
                                          {"agent_id": 4, "status": state}]}, agents, [3, 4], 200)
        coverage = acceptance.verify_status_coverage([{"status_samples": samples}], agents)
        self.assertEqual(coverage, {"cursor": ["idle", "working"], "claude": ["idle", "working"]})

    def test_poll_keeps_same_request_and_observes_working_then_idle(self):
        self.assertTrue(hasattr(acceptance, "poll_delivery"))
        statuses = iter(("working", "idle"))
        current = {"status": "working"}
        case = {"token": "TOKEN", "recipient_ids": [3], "receipt": {"message_id": 8, "request_ids": [9]}}
        def bus(*argv, **kwargs):
            if argv[0] == "message":
                self.assertEqual(argv, ("message", "status", 8))
                current["status"] = next(statuses)
                done = current["status"] == "idle"
                return {"complete": done, "requests": [{"agent_id": 3, "request_id": 9,
                        "stage": "replied" if done else "delivered", "start_bound": True,
                        "session_id": "session", "reply": {"text": "TOKEN"} if done else None}]}
            self.assertEqual(argv, ("diagnostics",))
            return {"agents": [{"agent_id": 3, "status": current["status"]}]}
        with mock.patch.object(acceptance.time, "sleep") as sleep:
            result = acceptance.poll_delivery(bus, case, [{"id": 3, "provider": "cursor"}], lambda: None)
        self.assertTrue(result["complete"])
        self.assertEqual([s["status"] for s in case["status_samples"]], ["working", "idle"])
        sleep.assert_called_once_with(0.2)


if __name__ == "__main__":
    unittest.main()
