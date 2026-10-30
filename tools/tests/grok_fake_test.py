"""Safety boundaries of the no-model Grok integration runner."""
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools.acceptance.grok_fake import Run


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
