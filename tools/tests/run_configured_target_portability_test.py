from __future__ import annotations

import errno
import os
import unittest
from unittest.mock import patch

from tools.tests import run_configured_target_test as configured_target


@unittest.skipUnless(os.name == "posix" and configured_target.REAL_CARGO, "requires POSIX and Cargo")
class RunConfiguredTargetPortabilityTests(unittest.TestCase):
    def test_configured_target_fixture_runs_without_a_macos_private_tmp_directory(self) -> None:
        make_temporary_directory = configured_target.tempfile.TemporaryDirectory

        def available_temporary_directory(*args, **kwargs):
            # A POSIX host such as Linux need not provide macOS's /private/tmp.
            parent = kwargs.get("dir")
            if parent is not None and os.fspath(parent) == "/private/tmp":
                raise FileNotFoundError(errno.ENOENT, "No such file or directory", "/private/tmp")
            return make_temporary_directory(*args, **kwargs)

        case = configured_target.RunConfiguredTargetTests(
            "test_dev_control_keeps_the_default_debug_binary_with_a_target_override"
        )
        result = unittest.TestResult()
        with patch.object(
            configured_target.tempfile,
            "TemporaryDirectory",
            side_effect=available_temporary_directory,
        ):
            case.run(result)
        self.assertEqual(result.testsRun, 1)
        self.assertEqual(result.errors, [], "the launcher fixture must run on supported POSIX hosts")
        self.assertEqual(result.failures, [])
