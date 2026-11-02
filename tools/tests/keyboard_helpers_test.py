"""Keyboard helper contracts with scripted stdin; no interactive TTY input."""
import json
from pathlib import Path
import select
import subprocess
import sys
from types import SimpleNamespace
import unittest
from unittest import mock

from tools.keyboard import capture_key_matrix, capture_keys, verify_suspicious_keys


SCRIPTS = (capture_key_matrix, capture_keys, verify_suspicious_keys)
READERS = ((capture_key_matrix, 0.025), (capture_keys, 0.020), (verify_suspicious_keys, 0.025))


class KeyboardHelperTests(unittest.TestCase):
    def test_formatters_keep_control_spellings_and_printable_bytes(self):
        data = b"\x00\x07\t\n\r\x1b A~\x7f\x80\xff"
        escaped = r"\x00\x07\t\n\r\x1b A~\x7f\x80\xff"
        for module in SCRIPTS:
            with self.subTest(script=module.__name__):
                self.assertEqual(module.to_hex(data), "0007090a0d1b20417e7f80ff")
                self.assertEqual(module.escaped(data), escaped)
                self.assertEqual(module.escaped(bytes(range(0x20, 0x7F))),
                                 "".join(chr(value) for value in range(0x20, 0x7F)))
                self.assertEqual(module.escaped(b""), "")

    def test_sanitize_preserves_the_existing_filename_slug_policy(self):
        for module in (capture_key_matrix, verify_suspicious_keys):
            for value, expected in (("", "terminal"), ("..._-", "terminal"),
                                    ("  WezTerm / tmux! ", "wezterm-tmux"),
                                    ("汉字", "terminal"), ("  A..B___C ", "a..b___c")):
                with self.subTest(script=module.__name__, value=value):
                    self.assertEqual(module.sanitize(value), expected)

    def test_reader_keeps_read_sizes_idle_boundary_and_split_bytes(self):
        for module, timeout in READERS:
            with self.subTest(script=module.__name__):
                buffer = mock.Mock(read1=mock.Mock(side_effect=[b"\x1b", b"[", b"A"]))
                stdin = SimpleNamespace(buffer=buffer)
                with mock.patch.object(sys, "stdin", stdin), \
                        mock.patch.object(select, "select", side_effect=[([stdin], [], []),
                                                                         ([stdin], [], []),
                                                                         ([], [], [])]) as wait:
                    self.assertEqual(module.read_sequence(), b"\x1b[A")
                self.assertEqual(buffer.read1.call_args_list,
                                 [mock.call(1), mock.call(1024), mock.call(1024)])
                self.assertEqual(wait.call_args_list, [mock.call([stdin], [], [], timeout)] * 3)

    def test_reader_eof_before_first_byte_does_not_wait(self):
        for module, _ in READERS:
            with self.subTest(script=module.__name__):
                buffer = mock.Mock(read1=mock.Mock(return_value=b""))
                with mock.patch.object(sys, "stdin", SimpleNamespace(buffer=buffer)), \
                        mock.patch.object(select, "select") as wait:
                    self.assertEqual(module.read_sequence(), b"")
                buffer.read1.assert_called_once_with(1)
                wait.assert_not_called()

    def test_reader_eof_after_first_byte_returns_that_byte(self):
        for module, timeout in READERS:
            with self.subTest(script=module.__name__):
                buffer = mock.Mock(read1=mock.Mock(side_effect=[b"x", b""]))
                stdin = SimpleNamespace(buffer=buffer)
                with mock.patch.object(sys, "stdin", stdin), \
                        mock.patch.object(select, "select", return_value=([stdin], [], [])) as wait:
                    self.assertEqual(module.read_sequence(), b"x")
                self.assertEqual(buffer.read1.call_args_list, [mock.call(1), mock.call(1024)])
                wait.assert_called_once_with([stdin], [], [], timeout)

    def test_reader_preserves_read_and_select_exceptions(self):
        for module, _ in READERS:
            for stage in ("first", "select", "continuation"):
                with self.subTest(script=module.__name__, stage=stage):
                    failure = OSError("scripted input failure")
                    reads = [failure] if stage == "first" else [b"x", failure]
                    buffer = mock.Mock(read1=mock.Mock(side_effect=reads))
                    stdin = SimpleNamespace(buffer=buffer)
                    with mock.patch.object(sys, "stdin", stdin), \
                            mock.patch.object(select, "select", side_effect=failure if stage == "select"
                                              else lambda *_: ([stdin], [], [])):
                        with self.assertRaisesRegex(OSError, "scripted input failure"):
                            module.read_sequence()

    def test_direct_script_imports_remain_noninteractive(self):
        code = """import json, runpy, sys
from pathlib import Path
path = Path(sys.argv[1])
sys.path.insert(0, str(path.parent))
module = runpy.run_path(str(path), run_name='keyboard_import_probe')
print(json.dumps({'hex': module['to_hex'](b'\\x1bA'), 'escaped': module['escaped'](b'\\x1bA')}))
"""
        for module in SCRIPTS:
            with self.subTest(script=module.__name__):
                result = subprocess.run([sys.executable, "-c", code, str(Path(module.__file__).resolve())],
                                        stdin=subprocess.DEVNULL, capture_output=True, text=True,
                                        timeout=10, check=True)
                self.assertEqual(json.loads(result.stdout), {"hex": "1b41", "escaped": r"\x1bA"})
                self.assertEqual(result.stderr, "")


if __name__ == "__main__":
    unittest.main()
