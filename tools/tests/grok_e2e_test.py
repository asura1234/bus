"""Synthetic adapter tests. No Bus process, provider or network is started."""
import unittest
from types import SimpleNamespace

from tools.acceptance import grok_provider


class GrokAcceptanceTests(unittest.TestCase):
    def test_grok_f_supported_worker_cases_are_not_blanket_skipped(self):
        for case in ("single", "queued", "steering", "dialog", "orchestrator", "adoption", "resume"):
            with self.subTest(case=case):
                self.assertIsNone(grok_provider.case_support(case))

    def test_grok_f_native_background_and_unknown_cases_are_honest_skips(self):
        self.assertIn("PENDING_AUTH", grok_provider.case_support("background"))
        self.assertIsNotNone(grok_provider.case_support("invented"))

    def test_grok_f_dialog_requests_a_question_without_filesystem_mutation(self):
        prompt = grok_provider.dialog_prompt("SYNTHETIC_TOKEN")
        self.assertIn("SYNTHETIC_TOKEN", prompt)
        self.assertIn("ask_user_question", prompt)
        self.assertNotIn("ln -s", prompt)

    def test_grok_f_worker_runner_uses_the_shared_scratch_roundtrip(self):
        calls = []
        run = SimpleNamespace(case_single=lambda provider, row: calls.append((provider, row)))
        row = {}
        grok_provider.run_case(run, "single", row)
        self.assertEqual(calls, [("grok", row)])

    def test_grok_f_resume_requires_the_original_session_and_new_terminal(self):
        for new_session, terminal in (("other", "new"), ("old", "old-terminal")):
            run = SimpleNamespace(
                sessions={"grok": "old"}, args=SimpleNamespace(timeout=1),
                report={"setup": {"grok": {"identity": {"terminal_id": "old-terminal"}}},
                        "resume": {"sessions_before": {"grok": "old"},
                                   "ready": {"grok": {"identity": {"terminal_id": terminal}}}}},
                round_trip=lambda *a, **kw: None)
            run.sessions["grok"] = new_session
            with self.subTest(session=new_session, terminal=terminal):
                with self.assertRaises(AssertionError):
                    grok_provider.run_case(run, "resume", {})

    def test_grok_f_resume_preserves_shared_harness_proof(self):
        calls = []
        run = SimpleNamespace(
            sessions={"grok": "old"},
            report={"setup": {"grok": {"identity": {"terminal_id": "before"}}},
                    "resume": {"sessions_before": {"grok": "old"},
                               "ready": {"grok": {"identity": {"terminal_id": "after"}}}}},
            round_trip=lambda *a, **kw: calls.append((a, kw)))
        grok_provider.run_case(run, "resume", {})
        self.assertEqual(calls[0][0][0], "grok")

    def test_grok_f_adoption_without_orchestrator_session_fails_before_mutation(self):
        with self.assertRaises(AssertionError):
            grok_provider.run_case(SimpleNamespace(sessions={}), "adoption", {})


if __name__ == "__main__":
    unittest.main()
