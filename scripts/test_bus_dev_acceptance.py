"""Acceptance-driver profiles and result validation; no live model calls."""
import contextlib
import io
import unittest

from scripts import bus_dev_acceptance as acceptance


class AcceptanceProfileTests(unittest.TestCase):
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


if __name__ == "__main__":
    unittest.main()
