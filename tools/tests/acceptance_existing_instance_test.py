"""Acceptance-driver profiles and result validation; no live model calls."""
import contextlib
import io
import json
from pathlib import Path
import tempfile
import threading
import unittest
from unittest import mock

from tools.acceptance import existing_instance_test as acceptance
from tools.acceptance import e2e_test as e2e


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

    def test_round_trip_prompt_requires_a_foreground_correlated_reply(self):
        prompt = acceptance.round_trip_prompt("TOKEN")
        self.assertIn("only testing Bus message round trip", prompt)
        self.assertIn("bus send --reply-to", prompt)
        self.assertIn("foreground", prompt)
        self.assertIn("--text TOKEN", prompt)
        self.assertIn("message ID", prompt)
        self.assertIn('stage:"replied"', prompt)
        self.assertNotIn("Do not use skills, tools", prompt)
        self.assertIn("Do not perform project work", prompt)

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
        # 测试在 tools/tests/ 下，仓库根是上两级。
        root = Path(__file__).resolve().parents[2]
        guide = (root / "orchestration/how-to-bus-cli.md").read_text()
        self.assertLessEqual(len(guide.splitlines()), 250)
        self.assertIn("bus <cmd> --help", guide)
        self.assertIn("agent read AGENT --source visible", guide)
        self.assertIn("agent read AGENT --source recent --lines N", guide)
        self.assertNotIn("returns up to 400 recent lines", guide)
        self.assertIn("agent observe AGENT", guide)
        self.assertIn("agent press AGENT --select", guide)
        self.assertIn("--terminal-id", guide)
        self.assertNotIn("agent choose", guide)
        self.assertNotIn("--fingerprint", guide)
        self.assertNotIn("approve-once", guide)
        self.assertIn("request recover REQUEST_ID --confirm", guide)
        self.assertNotIn("agent orchestrate", guide)
        self.assertIn("agent add --room master ... --orchestrates ROOM", guide)
        self.assertIn("same persisted message status", guide)
        self.assertIn("waiting CLI process exits or Bus restarts", guide)
        self.assertIn("Every session has exactly one MASTER room", guide)
        self.assertIn("Any agent can message any other agent", guide)
        self.assertIn("name (room R):", guide)
        self.assertIn('do not add a "from X", "X here" or signature line', guide)
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
    return {"text": text + "\n" + "\n".join(
        ("❯ " if n == 1 else "  ") + f"{n}. {label}" for n, label in enumerate(labels, 1)),
        "focus": [1, labels[0]], "terminal_id": "terminal-fixture", "content_revision": 6}


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
        self.assertIn("orchestrator case", e2e.case_support("codex", "adoption", ["codex"], ["adoption"]))
        self.assertIsNone(e2e.case_support("claude", "adoption", ["claude"], ["orchestrator", "adoption"]))
        # Bus refuses to add a Codex orchestrator, so there is no session to adopt either.
        for case in ("orchestrator", "adoption"):
            self.assertIn("refuses Codex", e2e.case_support("codex", case, ["codex"], ["orchestrator", "adoption"]))
            self.assertIsNone(e2e.case_support("cursor", case, ["cursor"], ["orchestrator", "adoption"]))


class EndToEndEnvironmentTests(unittest.TestCase):
    def test_provider_path_exposes_selected_executables_and_bus_without_lark(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            providers = root / "providers"
            providers.mkdir()
            for name in ("claude", "codex", "node", "lark-cli"):
                executable = providers / name
                executable.write_text("#!/bin/sh\nexit 0\n")
                executable.chmod(0o700)
            run = e2e.Run.__new__(e2e.Run)
            run.args = mock.Mock(providers=["claude", "codex"])
            run.binary, run.run_dir = root / "copied-bus", root / "report"
            run.run_dir.mkdir()
            run.env = {"PATH": str(providers)}
            run.bus, run.report = mock.Mock(env=dict(run.env)), {}
            run.prepare_provider_path()
            self.assertIsNone(e2e.shutil.which("lark-cli", path=run.env["PATH"]))
            self.assertEqual(run.bus.env["PATH"], run.env["PATH"])
            for name in ("claude", "codex", "node"):
                selected = Path(e2e.shutil.which(name, path=run.env["PATH"]))
                self.assertEqual(selected.resolve(), (providers / name).resolve())
            self.assertEqual((run.run_dir / "bin/bus").resolve(), run.binary.resolve())

    def test_live_run_preserves_login_home_and_uses_only_private_bus_state(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            login_home = root / "login-home"
            login_home.mkdir()
            args = mock.Mock(binary="stub-bus", providers=["claude", "codex"], cases=["single"])
            inherited = {"HOME": str(login_home), "USERPROFILE": str(login_home), "PATH": "/bin",
                         "BUS_DATA_DIR": "/live-bus", "BUS_TUI_FAKE_AGENT": "1",
                         "TEST_BUS_PROTECTED_HOME": str(login_home), "CODEX_HOME": "/other-login"}
            with mock.patch.dict(e2e.os.environ, inherited, clear=True), \
                    mock.patch.object(e2e, "E2E", root / "reports"), \
                    mock.patch.object(e2e, "find_binary", return_value=Path("/stub/bus")):
                run = e2e.Run(args)
                try:
                    self.assertEqual(run.env["HOME"], str(login_home))
                    self.assertEqual(run.env["USERPROFILE"], str(login_home))
                    self.assertNotIn("TEST_BUS_PROTECTED_HOME", run.env)
                    self.assertNotIn("CODEX_HOME", run.env)
                    self.assertNotIn("BUS_TUI_FAKE_AGENT", run.env)
                    self.assertNotEqual(run.env["BUS_DATA_DIR"], "/live-bus")
                    self.assertEqual(run.data.stat().st_mode & 0o777, 0o700)
                    run.bus.prepare_skills()
                    self.assertEqual(list(login_home.iterdir()), [])
                finally:
                    run.data.rmdir()

    def test_inherited_bus_herdr_and_claude_code_variables_are_scrubbed(self):
        with mock.patch("tools.acceptance.harness_test.uuid.uuid4") as owner:
            owner.return_value.hex = "scratch-owner"
            env = e2e.scrubbed_env({"PATH": "/bin", "HOME": "/home/me", "BUS_DATA_DIR": "/developer", "BUS_DEV": "1",
                                    "BUS_TUI_OWNER": "developer-owner", "HERDR_SOCKET_PATH": "/sock", "HERDR_ENV": "1",
                                    "CLAUDE_CODE_ENTRYPOINT": "cli", "CLAUDECODE": "1", "CODEX_HOME": "/codex"}, "/run/data")
        self.assertEqual(env, {"PATH": "/bin", "HOME": "/run/data-home", "USERPROFILE": "/run/data-home",
                               "TEST_BUS_PROTECTED_HOME": "/home/me", "BUS_DATA_DIR": "/run/data", "TERM": "xterm-256color",
                               "BUS_TUI_OWNER": "scratch-owner"})

    def test_binary_is_the_debug_bus_build_or_an_explicit_path(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            debug = root / "target/debug"
            debug.mkdir(parents=True)
            with self.assertRaises(SystemExit):
                e2e.find_binary(root=root)
            for name in ("bus", "other"):
                (debug / name).write_text("#!/bin/sh\n")
                (debug / name).chmod(0o755)
            self.assertEqual(e2e.find_binary(root=root), (debug / "bus").resolve())
            self.assertEqual(e2e.find_binary(str(debug / "other"), root=root), (debug / "other").resolve())
            (debug / "bus").chmod(0o644)
            with self.assertRaises(SystemExit):
                e2e.find_binary(root=root)

    def test_run_data_dir_keeps_session_sockets_under_the_unix_limit_from_any_checkout(self):
        with tempfile.TemporaryDirectory() as root:
            checkout = Path(root) / ("worktree-" + "x" * 80)
            args = mock.Mock(binary="stub-bus", providers=["claude"], cases=["single"])
            with mock.patch.object(e2e, "ROOT", checkout), \
                    mock.patch.object(e2e, "E2E", checkout / "temp/e2e"), \
                    mock.patch.object(e2e, "find_binary", return_value=Path("/stub/bus")):
                run = e2e.Run(args)
                data = Path(run.bus.env["BUS_DATA_DIR"])
                try:
                    for name in ("herdr-client.sock", "herdr.sock"):
                        socket = data / "herdr-config/sessions/bus" / name
                        self.assertLess(len(str(socket).encode()), 104, str(socket))
                    self.assertFalse(data.is_relative_to(e2e.ROOT), str(data))
                    self.assertEqual(data.stat().st_mode & 0o777, 0o700)
                finally:
                    if data.exists():
                        data.rmdir()

    def test_bus_launches_in_dev_mode(self):
        self.assertEqual(e2e.bus_argv(Path("/x/bus")), ["/x/bus", "--dev"])


class EndToEndDialogTests(unittest.TestCase):
    def test_grok_busy_followup_checks_queued_separate_turns_and_both_tokens(self):
        run = e2e.Run.__new__(e2e.Run)
        run.agents = {"grok": 3}
        run.wait_ready, run.verify = mock.Mock(), mock.Mock()
        run.token = mock.Mock(side_effect=["OLD", "NEW"])
        run.send = mock.Mock(side_effect=[{"message_id": 10}, {"message_id": 12}])
        run.bus = mock.Mock()
        first = {"agent_id": 3, "request_id": 11, "stage": "delivered", "turn_id": "first"}
        second = {"agent_id": 3, "request_id": 13, "group": None,
                  "turn_id": "second", "start_bound": True}
        run.bus.cli.side_effect = [{"complete": False, "requests": [first]},
                                   {"agents": [{"agent_id": 3, "status": "working"}]}]
        statuses = {10: {"requests": [first]}, 12: {"requests": [second]}}
        run.settle = mock.Mock(return_value=statuses)
        with mock.patch.object(e2e.time, "sleep"):
            run.case_steering("grok", {})
        self.assertIn("--text NEW", run.send.call_args.args[1])
        self.assertEqual([call.args[1] for call in run.verify.call_args_list], [{3: "OLD"}, {3: "NEW"}])

    def test_new_round_trip_waits_for_the_previous_native_turn_to_end(self):
        run = e2e.Run.__new__(e2e.Run)
        run.agents = {"claude": 3}
        run.token = mock.Mock(return_value="TOKEN")
        active = [True]  # The previous explicit reply settled before its native turn ended.
        run.wait_ready = mock.Mock(side_effect=lambda *a, **kw: active.__setitem__(0, False))
        def send(*a, **kw):
            self.assertFalse(active[0], "new task would steer the previous native turn")
            return {"message_id": 4}
        run.send = mock.Mock(side_effect=send)
        status = {"complete": True, "requests": []}
        run.settle = mock.Mock(return_value={4: status})
        run.verify = mock.Mock()
        self.assertIs(run.round_trip("claude", {}, "single"), status)

    def test_every_case_prompt_requires_a_foreground_correlated_reply(self):
        prompts = {provider: e2e.dialog_prompt(provider, "TOKEN") for provider in e2e.PROVIDERS}
        prompts.update(steering=e2e.steering_task("TOKEN"),
                       correction=e2e.steering_correction("OLD", "TOKEN"),
                       background=e2e.background_prompt("TOKEN"),
                       attachment=e2e.attachment_prompt("TOKEN"))
        for name, prompt in prompts.items():
            with self.subTest(case=name):
                self.assertIn("bus send --reply-to", prompt)
                self.assertIn("--text TOKEN", prompt)
                self.assertIn("foreground", prompt)

    def test_dialog_answer_uses_driver_selection_bound_to_observed_terminal(self):
        run = e2e.Run.__new__(e2e.Run)
        run.answered_frames = {}
        run.agents = {"claude": 3}
        run.lock = threading.Lock()
        run.report = {"dialogs": []}
        observed = {"terminal_id": "terminal-3", "pane_id": "w1:p2",
                    "text": "Accessing workspace:\n❯ 1. No, exit\n  2. Yes, I trust this folder\nEnter to select · Esc to cancel",
                    "focus": [1, "No, exit"], "content_revision": 2}
        calls = []

        def cli(*args, **kwargs):
            calls.append(args)
            if len(calls) == 1:
                self.assertEqual(args, ("agent", "observe", 3))
                return observed
            self.assertEqual(args, ("agent", "press", 3, "--select", "Yes, I trust this folder",
                                    "--terminal-id", "terminal-3"))
            return {"outcome": "selected", "keys": ["down", "enter"],
                    "selection_pending": False, "observation": observed}

        run.bus = mock.Mock(cli=cli)
        self.assertTrue(run.answer_dialogs("claude", "trust"))
        self.assertEqual(len(calls), 2)
        self.assertEqual(run.report["dialogs"][0]["label"], "Yes, I trust this folder")

    def test_answered_frame_is_not_confirmed_again_before_redraw(self):
        run = e2e.Run.__new__(e2e.Run)
        run.answered_frames = {}
        run.agents = {"codex": 3}
        run.lock = __import__("threading").Lock()
        run.report = {"dialogs": []}
        observed = dialog("Would you like to run the following command?", "Yes, proceed (y)", "No (esc)")
        calls = []

        def cli(*args, **kwargs):
            calls.append(args)
            return observed if args[1] == "observe" else {"outcome": "selected", "selection_pending": False}

        run.bus = mock.Mock(cli=cli)
        self.assertTrue(run.answer_dialogs("codex", "allow"))
        self.assertFalse(run.answer_dialogs("codex", "allow"))
        self.assertEqual([call[1] for call in calls], ["observe", "press", "observe"])

    def test_pending_driver_selection_fails_without_resending(self):
        run = e2e.Run.__new__(e2e.Run)
        run.answered_frames = {}
        run.agents = {"codex": 3}
        run.lock = __import__("threading").Lock()
        run.report = {"dialogs": []}
        observed = dialog("Would you like to run the following command?", "Yes, proceed (y)", "No (esc)")
        calls = []

        def cli(*args, **kwargs):
            calls.append(args)
            return observed if args[1] == "observe" else {"outcome": "selection_pending", "keys": ["down"]}

        run.bus = mock.Mock(cli=cli)
        with self.assertRaisesRegex(AssertionError, "inspect before continuing"):
            run.answer_dialogs("codex", "allow")
        self.assertEqual([call[1] for call in calls], ["observe", "press"])

    def test_fake_card_driver_preserves_uncertain_receipt_without_retry(self):
        from tools.acceptance.grok_fake import Run
        run = Run.__new__(Run)
        calls = []

        def cli(*args):
            calls.append(args)
            return {"outcome": "unknown", "keys": ["1"], "reason": "lost reply"}

        run.cli = cli
        with self.assertRaisesRegex(AssertionError, "without replaying"):
            run.driver_action(9, {"terminal_id": "fixture-terminal"}, "--key", "1")
        self.assertEqual(calls, [("agent", "press", 9, "--terminal-id", "fixture-terminal", "--key", "1")])

    def test_dialog_poll_reobserves_a_rejected_race_but_propagates_uncertainty(self):
        run = e2e.Run.__new__(e2e.Run)
        run.agents = {"claude": 3}
        run.bus = mock.Mock()
        run.bus.cli.side_effect = e2e.CliError({"code": "command_failed", "message": "Terminal changed; observe it again"})
        self.assertFalse(run.answer_dialogs("claude", "allow"))
        run.bus.cli.side_effect = e2e.CliError({"code": "command_failed", "message": "outcome uncertain"})
        with self.assertRaises(e2e.CliError):
            run.answer_dialogs("claude", "allow")

    def test_visible_numbered_dialog_can_select_when_global_focus_is_ambiguous(self):
        run = e2e.Run.__new__(e2e.Run)
        run.answered_frames, run.agents = {}, {"claude": 3}
        run.lock, run.report = threading.Lock(), {"dialogs": []}
        run.bus = mock.Mock()
        observation = {"text": "❯ Earlier prompt\nContinue the Bus e2e check?\n❯ 1. Yes\n  2. No\nEnter to select",
                       "focus": None, "terminal_id": "private-menu", "content_revision": 8}
        run.bus.cli.side_effect = [observation, {"outcome": "selected", "selection_pending": False}]
        self.assertTrue(run.answer_dialogs("claude", "allow"))
        self.assertEqual(run.bus.cli.call_args.args, ("agent", "press", 3, "--select", "Yes",
                                                    "--terminal-id", "private-menu"))

    def test_idle_observation_enqueues_no_input(self):
        run = e2e.Run.__new__(e2e.Run)
        run.answered_frames = {}
        run.agents = {"claude": 3}
        run.bus = mock.Mock()
        run.bus.cli.return_value = {"text": "> An editable composer", "focus": None}
        self.assertFalse(run.answer_dialogs("claude", "trust"))
        run.bus.cli.assert_called_once_with("agent", "observe", 3, row=None)

    def test_unnumbered_trust_menu_can_choose_the_unfocused_label(self):
        observation = {"text": "Trust this directory?\n  Trust and continue\n● Exit\n↑↓ navigate · Enter select · Esc exit",
                       "focus": [2, "Exit"]}
        self.assertEqual(e2e.pick_option(observation, "trust"), "Trust and continue")

    def test_trust_shortcut_label_stays_literal_for_the_driver(self):
        observation = {"text": "Workspace Trust Required\n  [a] Trust this workspace\n> [q] Quit\nEnter to select",
                       "focus": [2, "[q] Quit"]}
        self.assertEqual(e2e.pick_option(observation, "trust"), "[a] Trust this workspace")

    def test_trust_caption_above_numbered_options_is_not_a_choice(self):
        # Captured from the failed 20261102-171610 scratch run's setup_error.
        observation = {
            "cols": 131, "content_revision": 106,
            "cursor": {"col": 1, "row": 42, "visible": False},
            "focus": [7, "Trust and continue"], "focus_occurrence": 0,
            "pane_id": "w1:p3", "rows": 45, "terminal_id": "term_65f4798ea5f483",
            "text": "\n  Folder access\n"
                    "  /Users/dylanliu/work/bus-universal-dialog/temp/e2e/workspace\n\n"
                    "  Trust this folder? Codex can read, edit, and run files here, subject to your permission settings. Folder settings can run code\n"
                    "  automatically, even without a model request. Continue only if you trust these files. Your trust decision will be saved.\n\n"
                    "› 1. Trust and continue\n  2. Quit\n\n  enter continue · esc quit\n" + "\n" * 33,
        }
        self.assertEqual(e2e.pick_option(observation, "trust"), "Trust and continue")

    def test_permission_caption_above_numbered_options_is_not_a_choice(self):
        observed = dialog("Allow this command to edit the scratch file.", "Approve once", "No, cancel")
        self.assertEqual(e2e.pick_option(observed, "allow"), "Approve once")

    def test_hook_trust_selects_the_authorized_positive_choice(self):
        # Captured own Bus-hook setup; trusting those hooks was explicitly authorized.
        observation = {
            "cols": 121, "content_revision": 138,
            "cursor": {"col": 1, "row": 43, "visible": False},
            "focus": [6, "Review hooks"], "focus_occurrence": 0,
            "pane_id": "w1:p3", "rows": 46, "terminal_id": "term_65f476a89591a3",
            "text": "\n  Hooks need review\n  3 hooks are new or changed.\n"
                    "  Hooks can run outside the sandbox after you trust them.\n\n\n"
                    "› 1. Review hooks\n  2. Trust all and continue\n"
                    "  3. Continue without trusting (hooks won't run)\n\n"
                    "  enter confirm · esc skip" + "\n" * 35,
        }
        self.assertEqual(e2e.pick_option(observation, "trust"), "Trust all and continue")

    def test_non_trust_choice_does_not_resolve_multiple_positive_trust_choices(self):
        observed = dialog("Trust this directory?", "Trust without changing settings", "Yes, continue",
                          "Continue without trusting")
        with self.assertRaisesRegex(AssertionError, "ambiguous"):
            e2e.pick_option(observed, "trust")

    def test_multiple_trust_options_remain_ambiguous_with_or_without_numbers(self):
        for observed in (
            dialog("Trust this directory?", "Trust and continue", "Yes, continue", "Quit"),
            {"text": "Trust this directory?\n❯ Trust and continue\n  Yes, continue\n  Quit",
             "focus": [1, "Trust and continue"]},
        ):
            with self.subTest(observed=observed), self.assertRaisesRegex(AssertionError, "ambiguous"):
                e2e.pick_option(observed, "trust")

    def test_ambiguous_permission_labels_are_rejected(self):
        observed = dialog("Would you like to run the following command?", "Yes, proceed", "Approve once")
        with self.assertRaisesRegex(AssertionError, "ambiguous"):
            e2e.pick_option(observed, "allow")

    def test_trust_prompts_are_accepted_and_update_prompts_skipped(self):
        claude = dialog("Accessing workspace:", "No, exit", "Yes, I trust this folder")
        self.assertEqual(e2e.pick_option(claude, "trust"), "Yes, I trust this folder")
        self.assertEqual(e2e.pick_option(dialog("Do you trust this directory?", "Yes, continue", "No, quit"),
                                         "trust"), "Yes, continue")
        update = dialog("Update available · 0.1 -> 0.2", "Update now (runs `npm install -g @openai/codex`)",
                        "Skip", "Skip until next version")
        self.assertEqual(e2e.pick_option(update, "trust"), "Skip")
        self.assertEqual(e2e.pick_option(update, "allow"), "Skip")

    def test_permission_prompts_are_allowed_once_never_always(self):
        codex = dialog("Would you like to run the following command?",
                       "Yes, and don't ask again for this command (p)", "Yes, proceed (y)", "No (esc)")
        self.assertEqual(e2e.pick_option(codex, "allow"), "Yes, proceed (y)")
        question = dialog("Continue the Bus e2e check?", "Yes Continue the check", "No Stop", "Type something.")
        self.assertEqual(e2e.pick_option(question, "allow"), "Yes Continue the check")

    def test_unexpected_dialogs_fail_instead_of_guessing(self):
        for purpose in ("trust", "allow"):
            with self.assertRaises(AssertionError):
                e2e.pick_option(dialog("Pick a model", "Opus", "Sonnet"), purpose)

    def test_prompts_carry_the_token(self):
        for provider in e2e.PROVIDERS:
            self.assertIn("--text TOKEN", e2e.dialog_prompt(provider, "TOKEN"))
        self.assertIn("AskUserQuestion", e2e.dialog_prompt("claude", "T"))
        self.assertIn("escalated", e2e.dialog_prompt("codex", "T"))
        self.assertIn("run_in_background", e2e.background_prompt("T"))
        # Claude Code drops blank lines once it lifts an attached image; the prompt keeps one.
        self.assertEqual(e2e.attachment_prompt("T").count("\n\n"), 1)
        self.assertIn("\n\nUse your shell tool", e2e.attachment_prompt("T"))
        # Orchestrators run the test command themselves only when the developer says so.
        for prompt in (e2e.steering_task, e2e.background_prompt):
            self.assertNotIn("I authorize", prompt("T"))
            self.assertIn("I authorize", prompt("T", True))
        self.assertIn("every Bus message ID", e2e.steering_correction("OLD", "NEW"))
        background = e2e.background_prompt("T", room="master")
        self.assertIn("bus send --room master", background)
        self.assertIn("--to you --text FOLLOWUP_T", background)

    def test_orchestrator_adoption_args_and_image(self):
        self.assertEqual(e2e.adopt_args("codex", "S"), "resume S")
        self.assertEqual(e2e.adopt_args("claude", "S"), "--resume S")
        self.assertEqual(e2e.adopt_args("cursor", "S"), "--resume S")
        self.assertEqual(e2e.orchestrator_key("cursor"), "orch-cursor")
        with tempfile.TemporaryDirectory() as tmp:
            data = e2e.write_png(Path(tmp) / "x.png").read_bytes()
        self.assertTrue(data.startswith(b"\x89PNG\r\n\x1a\n"))
        self.assertTrue(data.endswith(b"IEND\xaeB`\x82"))


class EndToEndStartupTests(unittest.TestCase):
    # Actual non-dialog startup observation and diagnostic from 20261102-174001.
    STARTUP_OBSERVATION = {
        "cols": 131, "content_revision": 24,
        "cursor": {"col": 25, "row": 42, "visible": False},
        "focus": [41, 'Try "create a util logging.py that..."'], "focus_occurrence": 0,
        "pane_id": "w1:p2", "rows": 45, "terminal_id": "term_65f47ee3a1e222",
        "text": ("\n ▐▛███▛█   Claude Code v2.1.316\n▝▜██████▀  Opus 5.5 · Claude Team\n"
                 " ▝▝   ▝▝   ~/work/bus-universal-dialog/temp/e2e/workspace\n"
                 + "\n" * 35 + " " * 111 + "◐ medium · /effort\n"
                 + "─" * 131 + '\n❯\xa0Try "create a util logging.py that..."\n'
                 + "─" * 25 + "\n\n"),
    }
    STARTUP_WAITING = {
        "agent_id": 3, "current_request": None,
        "detail": "Not ready; waiting for provider hook readiness.",
        "identity": {"launch_id": "8cb0ad9b6e029d2c53a89029c4ded766abec9e03e83d27a91ab13220c5e65300",
                     "pane_id": "w1:p2", "session_id": None, "terminal_id": "term_65f47ee3a1e222"},
        "name": "claude", "reason": "agent_launching", "status": "launching",
    }

    def make_run(self, samples, observation):
        run = e2e.Run.__new__(e2e.Run)
        run.agents = {"claude": 3}
        run.answered_frames = {}
        run.lock = __import__("threading").Lock()
        run.report = {"dialogs": []}
        run.save = lambda: None
        samples = iter(samples)
        current = {}
        calls, inputs = [], []

        def cli(*argv, **_kwargs):
            calls.append(argv)
            if argv == ("diagnostics",):
                current["diagnostics"], current["shown_status"] = next(samples)
                return {"agents": [current["diagnostics"]]}
            if argv == ("state",):
                return {"agents": [{"id": 3, "status": current["shown_status"]}]}
            if argv == ("agent", "observe", 3):
                return observation
            if argv[:3] == ("agent", "press", 3):
                inputs.append(argv)
                return {"outcome": "selected", "selection_pending": False,
                        "keys": ["down", "enter"], "observation": observation}
            raise AssertionError(f"unexpected startup call: {argv}")

        run.bus = mock.Mock(cli=cli)
        return run, calls, inputs

    def ready_diagnostics(self):
        return {**self.STARTUP_WAITING, "reason": None, "status": "idle",
                "identity": {**self.STARTUP_WAITING["identity"], "session_id": "ready-session"}}

    def test_focused_composer_waits_for_hook_readiness_without_input(self):
        ready = self.ready_diagnostics()
        run, calls, inputs = self.make_run(
            [(self.STARTUP_WAITING, "launching"), (ready, "idle")], self.STARTUP_OBSERVATION)
        record = {}
        with mock.patch.object(e2e.time, "sleep"):
            run.wait_ready(["claude"], record)
        self.assertEqual(record["claude"]["identity"], ready["identity"])
        self.assertEqual(calls.count(("diagnostics",)), 2)
        self.assertEqual(run.report["dialogs"], [])
        self.assertEqual(inputs, [])

    def test_public_blocked_trust_is_answered_while_diagnostics_still_launches(self):
        observed = dialog("Accessing workspace:", "No, exit", "Yes, I trust this folder")
        ready = self.ready_diagnostics()
        run, _calls, inputs = self.make_run(
            [(self.STARTUP_WAITING, "blocked"), (ready, "idle")], observed)
        record = {}
        with mock.patch.object(e2e.time, "sleep"):
            run.wait_ready(["claude"], record)
        self.assertEqual(record["claude"]["identity"], ready["identity"])
        self.assertEqual(run.report["dialogs"][0]["label"], "Yes, I trust this folder")
        self.assertEqual(inputs, [("agent", "press", 3, "--select", "Yes, I trust this folder",
                                   "--terminal-id", "terminal-fixture")])

    def test_public_blocked_trust_is_not_ready_when_raw_diagnostics_is_idle(self):
        observed = dialog("Accessing workspace:", "No, exit", "Yes, I trust this folder")
        ready = self.ready_diagnostics()
        run, calls, inputs = self.make_run([(ready, "blocked"), (ready, "idle")], observed)
        with mock.patch.object(e2e.time, "sleep"):
            run.wait_ready(["claude"], {})
        self.assertEqual(calls.count(("diagnostics",)), 2)
        self.assertEqual(len(inputs), 1)
        self.assertEqual(run.report["dialogs"][0]["label"], "Yes, I trust this folder")

    def test_public_blocked_unsupported_dialog_still_fails_without_input(self):
        observed = dialog("Pick a model", "Opus", "Sonnet")
        run, _calls, inputs = self.make_run([(self.STARTUP_WAITING, "blocked")], observed)
        with mock.patch.object(e2e.time, "sleep"), self.assertRaisesRegex(AssertionError, "unexpected trust"):
            run.wait_ready(["claude"], {})
        self.assertEqual(inputs, [])
        self.assertEqual(run.report["dialogs"], [])

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
        run.answered_frames = {}
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


class EndToEndBackgroundReplyTests(unittest.TestCase):
    def test_idle_without_an_authored_background_followup_does_not_pass(self):
        reply = {"callback_id": "reply-to:44", "text": "TOKEN", "provider_session_id": "session",
                 "provider_turn_id": "initial-turn", "received_at_ms": 10}
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            stored = {"completed_reply": {"content": reply}}
            (root / "state.json").write_text(json.dumps({"state": {"requests": {"44": stored}}}))
            run = e2e.Run.__new__(e2e.Run)
            run.data, run.room, run.args, run.agents = root, 2, mock.Mock(timeout=1), {"claude": 3}
            run.round_trip = mock.Mock(return_value={"requests": [{"request_id": 44, "reply": reply}]})
            run.answer_dialogs = mock.Mock(side_effect=AssertionError("idle composer is not a dialog"))
            run.bus = mock.Mock()
            run.bus.cli.side_effect = lambda command, *a, **kw: (
                {"agents": [{"agent_id": 3, "status": "idle", "reason": None}]}
                if command == "diagnostics" else {"messages": []})
            with mock.patch.object(e2e.time, "monotonic", side_effect=[0, 2]), \
                    self.assertRaisesRegex(AssertionError, "agent stuck after background shell"):
                run.case_background("claude", e2e.new_row("claude", "background"))
            self.assertEqual(run.round_trip.call_count, 1)
            run.answer_dialogs.assert_not_called()

    def test_explicit_background_reply_is_valid_before_its_native_turn_ends(self):
        reply = {"callback_id": "reply-to:44", "text": "TOKEN", "provider_session_id": "session",
                 "provider_turn_id": "initial-turn", "received_at_ms": 10}
        request = {"phase": "active", "pending_final": None,
                   "completed_reply": {"content": reply, "group": None}}
        self.assertEqual(e2e.verify_background_reply(request, reply), {"initial_reply": reply})

    def test_background_explicit_answer_survives_native_completion(self):
        reply = {"callback_id": "reply-to:44", "text": "TOKEN", "provider_session_id": "session",
                 "provider_turn_id": "initial-turn", "received_at_ms": 10}
        request = {"phase": "completed", "pending_final": None, "completed_reply": {"content": reply}}
        before = json.dumps(request, sort_keys=True)
        self.assertEqual(e2e.verify_background_reply(request, reply), {"initial_reply": reply})
        self.assertEqual(json.dumps(request, sort_keys=True), before)
        for change in ({"text": "FOLLOWUP_TOKEN"}, {"provider_turn_id": "wake-turn"},
                       {"provider_session_id": "other"}, {"received_at_ms": 20}):
            with self.subTest(change=change), self.assertRaises(AssertionError):
                e2e.verify_background_reply(request, {**reply, **change})
        with self.assertRaises(AssertionError):
            e2e.verify_background_reply({"phase": "active", "pending_final": reply}, reply)


if __name__ == "__main__":
    unittest.main()
