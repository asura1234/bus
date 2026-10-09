import datetime as dt
import io
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest import mock

from tools import bus_screenshot as shot

W = shot.Window


class TitleTest(unittest.TestCase):
    def test_bus_client_titles_match_and_repo_shells_do_not(self) -> None:
        for title in ("bogon: bus", "bus", "host: bus --dev"):
            self.assertTrue(shot.is_bus_title(title), title)
        for title in (
            "~/work/bus",
            "dylanliu@bogon:~/work/bus",
            "bogon: business",
            "claude",
            # Legacy branding must not select a window after the cutover.
            "herdr",
            "host: herdr --dev",
        ):
            self.assertFalse(shot.is_bus_title(title), title)


class ChooseWindowTest(unittest.TestCase):
    windows = [
        W(1, "Google Chrome", "bus docs", True),
        W(2, "iTerm2", "~/work/bus", True),
        W(3, "Ghostty", "bogon: bus", False),
        W(4, "iTerm2", "bogon: bus", True),
        W(5, "Ghostty", "bogon: bus", True),
    ]

    def test_prefers_the_frontmost_onscreen_terminal_running_bus(self) -> None:
        self.assertEqual(shot.choose_window(self.windows).id, 4)

    def test_explicit_window_id_and_title_override_the_default(self) -> None:
        self.assertEqual(shot.choose_window(self.windows, window_id=3).id, 3)
        self.assertEqual(shot.choose_window(self.windows, title="~/work").id, 2)
        with self.assertRaisesRegex(shot.ScreenshotError, "No window with id 9"):
            shot.choose_window(self.windows, window_id=9)

    def test_offscreen_or_missing_bus_window_is_a_clear_error(self) -> None:
        with self.assertRaisesRegex(shot.ScreenshotError, "not on screen"):
            shot.choose_window([W(3, "Ghostty", "bogon: bus", False)])
        with self.assertRaisesRegex(
            shot.ScreenshotError, "No iTerm2 or Ghostty window"
        ):
            shot.choose_window([W(1, "Google Chrome", "bogon: bus", True)])


class PermissionTest(unittest.TestCase):
    def test_hidden_titles_name_the_app_that_needs_screen_recording(self) -> None:
        windows = [W(1, "iTerm2", None, True), W(2, "Finder", None, True)]
        with mock.patch.dict("os.environ", {"TERM_PROGRAM": "ghostty"}):
            with self.assertRaisesRegex(
                shot.ScreenshotError, "Allow Ghostty in System Settings"
            ):
                shot.check_permission(windows)
        shot.check_permission([W(1, "iTerm2", "bogon: bus", True)])


class MainTest(unittest.TestCase):
    def run_main(self, argv, windows, capture=None):
        out, err = io.StringIO(), io.StringIO()
        with (
            mock.patch.object(shot, "list_windows", return_value=windows),
            mock.patch.object(
                shot,
                "capture",
                side_effect=capture or (lambda window, path: path.write_bytes(b"png")),
            ),
            redirect_stdout(out),
            redirect_stderr(err),
        ):
            code = shot.main(argv)
        return code, out.getvalue(), err.getvalue()

    def test_saves_a_timestamped_png_and_prints_its_path_last(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            code, out, _ = self.run_main(
                ["--out-dir", tmp], [W(4, "iTerm2", "bogon: bus", True)]
            )
            self.assertEqual(code, 0)
            path = Path(out.strip().splitlines()[-1])
            self.assertTrue(path.is_absolute() and path.is_file(), path)
            self.assertRegex(path.name, r"^bus-\d{8}-\d{6}\.png$")

    def test_failures_exit_nonzero_with_the_reason_on_stderr(self) -> None:
        code, out, err = self.run_main([], [W(2, "iTerm2", "~/work/bus", True)])
        self.assertEqual((code, out), (1, ""))
        self.assertIn("No iTerm2 or Ghostty window", err)

    def test_screenshot_path_uses_the_local_time(self) -> None:
        path = shot.screenshot_path(Path("/x"), dt.datetime(2026, 10, 8, 12, 5, 9))
        self.assertEqual(path, Path("/x/bus-20261008-120509.png"))


if __name__ == "__main__":
    unittest.main()
