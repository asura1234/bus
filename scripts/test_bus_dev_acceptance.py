"""Acceptance-driver profiles and result validation; no live model calls."""
import contextlib
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock

from scripts import bus_dev_acceptance as acceptance
from scripts import bus_e2e as e2e


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
        self.assertEqual(profile["messages"], 20)
        self.assertEqual(profile["recipients"], ((0,), (1,), (2,), (0, 1), (0, 2), (1, 2), (0, 1, 2)))

    def test_live_matrix_defaults_to_five_seconds_between_messages(self):
        args = acceptance.parse_args(["--allow-live-models", "--profile", "claude-codex-cursor",
                                      "--claude-pwd", "/tmp/claude", "--codex-pwd", "/tmp/codex",
                                      "--cursor-pwd", "/tmp/cursor"])
        self.assertEqual(args.message_delay, 5.0)

    def test_round_trip_prompt_explicitly_forbids_skills_tools_and_work(self):
        prompt = acceptance.round_trip_prompt("TOKEN")
        self.assertIn("only testing Bus message round trip", prompt)
        self.assertIn("Do not use skills, tools, or modify files", prompt)
        self.assertIn("Reply with exactly TOKEN", prompt)

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
        self.assertEqual(args.messages, 20)
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
    def test_public_guide_documents_explicit_reads_and_safe_room_recovery_surfaces(self):
        guide = (Path(__file__).resolve().parents[1] / "docs/how-to-bus-cli.md").read_text()
        self.assertIn("agent read AGENT --source visible", guide)
        self.assertIn("agent read AGENT --source recent --lines N", guide)
        self.assertNotIn("returns up to 400 recent lines", guide)
        self.assertIn("agent dialog AGENT", guide)
        self.assertIn("agent choose AGENT --option N --fingerprint FINGERPRINT", guide)
        self.assertNotIn("approve-once", guide)
        self.assertIn("request recover REQUEST_ID --confirm", guide)
        self.assertIn("agent orchestrate AGENT (--room ROOM | --none)", guide)
        self.assertIn("agent add --room master ... [--orchestrates ROOM]", guide)
        self.assertIn("same persisted message status", guide)
        self.assertIn("waiting CLI process exits or Bus restarts", guide)
        self.assertIn("Every session has exactly one MASTER room", guide)
        self.assertIn("or the room's MASTER\norchestrator", guide)
        self.assertIn("does not interpret the reply", guide)
        self.assertIn("send --wait", guide)

    def test_terminal_read_retries_transient_eagain(self):
        calls = []
        outputs = iter([
            AssertionError({"error": {"message": "Resource temporarily unavailable (os error 35)"}}),
            {"text": "TOKEN"},
        ])

        def bus(*args, **_kwargs):
            calls.append(args)
            result = next(outputs)
            if isinstance(result, Exception):
                raise result
            return result

        with mock.patch.object(acceptance.time, "sleep") as sleep:
            self.assertEqual(acceptance.read_agent_output(bus, 3, timeout=1), "TOKEN")
        sleep.assert_called_once()
        self.assertEqual(calls, [
            ("agent", "read", 3, "--source", "recent", "--lines", "80"),
            ("agent", "read", 3, "--source", "recent", "--lines", "80"),
        ])

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


def installed(*commands):
    return lambda command: "/bin/" + command if command in commands else None


def dialog(text, *labels):
    return {"text": text, "options": [{"number": n, "label": label, "selected": n == 1}
                                      for n, label in enumerate(labels, 1)]}


class EndToEndArgumentTests(unittest.TestCase):
    def parse(self, *argv, which=installed("claude", "codex", "cursor-agent")):
        with contextlib.redirect_stderr(io.StringIO()):
            return e2e.parse_args(list(argv), which=which)

    def test_live_models_must_be_allowed_explicitly(self):
        with self.assertRaises(SystemExit):
            self.parse()
        self.assertTrue(self.parse("--allow-live-models").allow_live_models)

    def test_defaults_to_installed_providers_and_every_case(self):
        args = self.parse("--allow-live-models", which=installed("codex", "claude"))
        self.assertEqual(args.providers, ["claude", "codex"])
        self.assertEqual(args.cases, list(e2e.CASES))
        self.assertFalse(args.keep)
        self.assertIsNone(args.binary)

    def test_selection_keeps_canonical_order_and_rejects_unknown_or_missing(self):
        args = self.parse("--allow-live-models", "--providers", "cursor,claude", "--cases", "resume, single")
        self.assertEqual((args.providers, args.cases), (["claude", "cursor"], ["single", "resume"]))
        for argv in (["--providers", "gemini"], ["--cases", "single,typo"], ["--cases", ","],
                     ["--timeout", "5"]):
            with self.assertRaises(SystemExit):
                self.parse("--allow-live-models", *argv)
        with self.assertRaises(SystemExit):
            self.parse("--allow-live-models", "--providers", "cursor", which=installed("claude"))

    def test_skip_reasons_for_unsupported_cases(self):
        self.assertIsNone(e2e.case_support("claude", "background", ["claude"]))
        self.assertIn("Claude", e2e.case_support("codex", "background", ["claude", "codex"]))
        self.assertIn("two providers", e2e.case_support("cursor", "multi", ["cursor"]))
        self.assertIsNone(e2e.case_support("cursor", "multi", ["claude", "cursor"]))


class EndToEndEnvironmentTests(unittest.TestCase):
    def test_inherited_bus_herdr_and_claude_code_variables_are_scrubbed(self):
        env = e2e.scrubbed_env({"PATH": "/bin", "HOME": "/home/me", "BUS_DATA_DIR": "/human", "BUS_DEV": "1",
                                "HERDR_SOCKET_PATH": "/sock", "HERDR_ENV": "1", "CLAUDE_CODE_ENTRYPOINT": "cli",
                                "CLAUDECODE": "1", "CODEX_HOME": "/codex"}, "/run/data")
        self.assertEqual(env, {"PATH": "/bin", "HOME": "/home/me", "CODEX_HOME": "/codex",
                               "BUS_DATA_DIR": "/run/data", "TERM": "xterm-256color"})

    def test_binary_prefers_bus_then_herdr_or_an_explicit_path(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            debug = root / "target/debug"
            debug.mkdir(parents=True)
            with self.assertRaises(SystemExit):
                e2e.find_binary(root=root)
            for name in ("herdr", "bus"):
                (debug / name).write_text("#!/bin/sh\n")
                (debug / name).chmod(0o755)
                self.assertEqual(e2e.find_binary(root=root), (debug / name).resolve())
            self.assertEqual(e2e.find_binary(str(debug / "herdr"), root=root), (debug / "herdr").resolve())
            (debug / "bus").chmod(0o644)
            self.assertEqual(e2e.find_binary(root=root), (debug / "herdr").resolve())

    def test_only_the_herdr_binary_needs_the_bus_flag(self):
        self.assertEqual(e2e.bus_argv(Path("/x/herdr")), ["/x/herdr", "--bus", "--dev"])
        self.assertEqual(e2e.bus_argv(Path("/x/bus")), ["/x/bus", "--dev"])


class EndToEndDialogTests(unittest.TestCase):
    def test_trust_prompts_are_accepted_and_update_prompts_skipped(self):
        claude = dialog("Accessing workspace:", "No, exit", "Yes, I trust this folder")
        self.assertEqual(e2e.pick_option(claude, "trust"), 2)
        self.assertEqual(e2e.pick_option(dialog("Do you trust this directory?", "Yes, continue", "No, quit"),
                                         "trust"), 1)
        update = dialog("Update available · 0.1 -> 0.2", "Update now (runs `npm install -g @openai/codex`)",
                        "Skip", "Skip until next version")
        self.assertEqual(e2e.pick_option(update, "trust"), 2)
        self.assertEqual(e2e.pick_option(update, "allow"), 2)

    def test_permission_prompts_are_allowed_once_never_always(self):
        codex = dialog("Would you like to run the following command?",
                       "Yes, and don't ask again for this command (p)", "Yes, proceed (y)", "No (esc)")
        self.assertEqual(e2e.pick_option(codex, "allow"), 2)
        question = dialog("Continue the Bus e2e check?", "Yes Continue the check", "No Stop", "Type something.")
        self.assertEqual(e2e.pick_option(question, "allow"), 1)

    def test_unexpected_dialogs_fail_instead_of_guessing(self):
        for purpose in ("trust", "allow"):
            with self.assertRaises(AssertionError):
                e2e.pick_option(dialog("Pick a model", "Opus", "Sonnet"), purpose)

    def test_prompts_carry_the_token(self):
        for provider in e2e.PROVIDERS:
            self.assertIn("reply with exactly TOKEN", e2e.dialog_prompt(provider, "TOKEN"))
        self.assertIn("AskUserQuestion", e2e.dialog_prompt("claude", "T"))
        self.assertIn("escalated", e2e.dialog_prompt("codex", "T"))
        self.assertIn("run_in_background", e2e.background_prompt("T"))


class EndToEndReportTests(unittest.TestCase):
    def test_rows_summary_and_table(self):
        rows = [e2e.new_row("claude", "single"), e2e.new_row("codex", "background"),
                e2e.new_row("cursor", "dialog")]
        self.assertEqual(set(rows[0]), {"provider", "case", "result", "seconds", "reason", "commands",
                                        "last_status"})
        self.assertEqual(rows[0]["result"], "FAIL")
        rows[0].update(result="PASS", seconds=3.25)
        rows[1].update(result="SKIP", seconds=0.0, reason="not supported")
        self.assertEqual(e2e.summarize(rows[:2]), {"passed": True, "PASS": 1, "FAIL": 0, "SKIP": 1})
        self.assertFalse(e2e.summarize(rows)["passed"])
        self.assertFalse(e2e.summarize(rows[1:2])["passed"], "all skipped is not a pass")
        table = e2e.format_table(rows).splitlines()
        self.assertEqual(table[0].split(), ["provider", "case", "result", "seconds", "detail"])
        self.assertEqual(table[1].split(), ["claude", "single", "PASS", "3.2"])
        self.assertEqual(table[2].split()[:4], ["codex", "background", "SKIP", "0.0"])
        self.assertEqual(table[3].split(), ["cursor", "dialog", "FAIL"])
        json.dumps(rows)

    def test_settle_answers_dialogs_and_keeps_the_last_status(self):
        run = e2e.Run.__new__(e2e.Run)
        run.args = mock.Mock(timeout=30)
        run.agents = {"claude": 3}
        statuses = iter([{"complete": False, "waiting_on_dialog": [3], "requests": []},
                         {"complete": True, "waiting_on_dialog": [], "requests": []}])
        run.bus = mock.Mock(cli=lambda *argv, **_: next(statuses))
        run.answer_dialogs = mock.Mock(return_value=True)
        row = e2e.new_row("claude", "dialog")
        with mock.patch.object(e2e.time, "sleep"):
            result = run.settle([7], row)
        self.assertTrue(result[7]["complete"])
        run.answer_dialogs.assert_called_once_with("claude", "allow", row)
        self.assertEqual(row["dialogs_answered"], 1)
        self.assertTrue(row["last_status"]["complete"])


if __name__ == "__main__":
    unittest.main()
