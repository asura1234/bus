"""`atomic_write` must pin canonical LF explicitly — otherwise the whole review chain is unusable on Windows.

`prepare_review_input.py` writes its sanitized output with `atomic_write`, and downstream consumes it as
canonical LF. Text mode's default `newline=None` translates `\\n` to the platform newline on write, giving
CRLF on Windows, so the artifact this chain writes on Windows disagrees with the canonical form it itself
declares — **the product is unusable, not a test problem**. This defect previously had no coverage at all:
`atomic_write` had not a single test.

Warning — **why this is a white-box assertion (checking the arguments passed to `os.fdopen`) rather than
reading the bytes back**: CPython's `TextIOWrapper` decides the write-side newline in C with a
**compile-time** `#ifdef MS_WINDOWS` and **does not read the runtime `os.linesep`**. So on a non-Windows
host the output is LF whether or not `newline` is passed — a byte assertion is always green on
macOS/Linux, and patching `os.linesep` to `"\\r\\n"` has no effect at all (that was the first version I
wrote; during mutation testing it failed to catch removing the fix, which is how I found it guarded
nothing).

The only invariant that holds on every host is this one: **newline must be pinned to LF explicitly here**.
White-box is the only honest choice here, not a shortcut.
"""

import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


REPO_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO_ROOT / "cli_extensions"))

from review_artifact import atomic_write  # noqa: E402


class AtomicWriteTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.tmp_path = Path(temporary.name)

    def test_atomic_write_pins_lf_newline_explicitly(self) -> None:
        seen: dict[str, object] = {}
        original = os.fdopen

        def recording(descriptor: int, mode: str = "r", *args, **kwargs):
            seen["mode"] = mode
            seen["newline"] = kwargs.get("newline")
            seen["encoding"] = kwargs.get("encoding")
            return original(descriptor, mode, *args, **kwargs)

        with mock.patch.object(os, "fdopen", recording):
            atomic_write(self.tmp_path / "sanitized.md", "first\nsecond\n")

        # Absent (None) means default platform translation — exactly the shape of the Windows defect.
        self.assertEqual(seen["newline"], "\n")
        # Encoding must be explicit too: the default locale encoding on Windows is cp936/cp1252 or
        # similar, while these artifacts' validation requires UTF-8.
        self.assertEqual(seen["encoding"], "utf-8")

    def test_atomic_write_content_round_trips(self) -> None:
        target = self.tmp_path / "sanitized.md"
        atomic_write(target, "first\nsecond\n")

        # By bytes rather than `read_text()`: reading in text mode normalizes CRLF to `\n`, so a CRLF
        # artifact looks perfectly normal under `read_text()` — the other half of why this kind of
        # defect slips through.
        # Warning: on a non-Windows host this **cannot** prove the fix works (see the module docstring);
        # it guards the content itself.
        self.assertEqual(target.read_bytes(), b"first\nsecond\n")

    def test_atomic_write_replaces_existing_and_leaves_no_temp(self) -> None:
        target = self.tmp_path / "sanitized.md"
        atomic_write(target, "old\n")
        atomic_write(target, "new\n")

        self.assertEqual(target.read_bytes(), b"new\n")
        # The temp file sits in the target's directory (`os.replace` requires the same filesystem), so
        # any leftover would remain here.
        self.assertEqual([entry.name for entry in self.tmp_path.iterdir()], ["sanitized.md"])


if __name__ == "__main__":
    unittest.main()
