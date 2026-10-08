#!/usr/bin/env python3
"""Capture the terminal window that runs the Bus client and print the PNG path.

macOS only. Windows are listed with CoreGraphics through osascript (JXA), so no
Python packages are needed, and the window is captured with `screencapture -x
-o -l ID`: silent, no shadow, and without bringing the window to the front.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
TERMINALS = ("iTerm2", "Ghostty")

# CGWindowListCopyWindowInfo lists windows front to back. Titles
# (kCGWindowName) are only visible with Screen Recording permission, which is
# also what tells a missing permission apart from a missing window.
LIST_WINDOWS_JXA = """
ObjC.import("CoreGraphics");
var raw = $.CGWindowListCopyWindowInfo($.kCGWindowListOptionAll, 0);
var windows = ObjC.deepUnwrap(ObjC.castRefToObject(raw)) || [];
JSON.stringify(windows.filter(function (w) { return w.kCGWindowLayer === 0; }).map(function (w) {
  return {id: w.kCGWindowNumber, owner: w.kCGWindowOwnerName || "",
          title: w.kCGWindowName === undefined ? null : w.kCGWindowName,
          onscreen: !!w.kCGWindowIsOnscreen};
}));
"""


class ScreenshotError(Exception):
    """A failure the user can act on; its message is printed as is."""


@dataclass(frozen=True)
class Window:
    id: int
    owner: str
    title: str | None
    onscreen: bool


def is_bus_title(title: str) -> bool:
    """The shell titles a window `host: command` while a command runs, so the
    Bus client's window ends in `: bus` (or `: bus --dev`). A shell that only
    sits in the repo shows its directory (`~/work/bus`) and must not match.
    `herdr` is the title Bus sets itself until the rename lands."""
    command = title.rsplit(": ", 1)[-1].strip()
    return re.fullmatch(r"(bus|herdr)(\s.*)?", command) is not None


def list_windows() -> list[Window]:
    result = subprocess.run(
        ["osascript", "-l", "JavaScript", "-e", LIST_WINDOWS_JXA],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        raise ScreenshotError(f"Could not list windows: {result.stderr.strip()}")
    return [Window(**window) for window in json.loads(result.stdout or "[]")]


def host_app() -> str:
    """The app that runs this command, which is the one macOS asks about."""
    program = os.environ.get("TERM_PROGRAM", "")
    return {
        "iTerm.app": "iTerm2",
        "ghostty": "Ghostty",
        "Apple_Terminal": "Terminal",
    }.get(program, program or "the terminal app that runs this command")


def check_permission(windows: list[Window]) -> None:
    if windows and all(window.title is None for window in windows):
        raise ScreenshotError(
            "No Screen Recording permission: window titles are hidden. Allow "
            f"{host_app()} in System Settings > Privacy & Security > Screen "
            "Recording, then restart it."
        )


def choose_window(
    windows: list[Window], window_id: int | None = None, title: str | None = None
) -> Window:
    """The Bus window to capture: the given id, else the frontmost on-screen
    iTerm2 or Ghostty window whose title contains `title` (or looks like the
    Bus client when no title is given)."""
    if window_id is not None:
        for window in windows:
            if window.id == window_id:
                return window
        raise ScreenshotError(f"No window with id {window_id}.")
    candidates = [
        window
        for window in windows
        if window.owner in TERMINALS
        and window.title is not None
        and (
            title.lower() in window.title.lower()
            if title
            else is_bus_title(window.title)
        )
    ]
    onscreen = [window for window in candidates if window.onscreen]
    if onscreen:
        return onscreen[0]
    wanted = f'titled like "{title}"' if title else 'running "bus"'
    if candidates:
        raise ScreenshotError(
            f"The {candidates[0].owner} window {wanted} is not on screen "
            "(minimized or on another Space); show it and retry."
        )
    raise ScreenshotError(
        f"No iTerm2 or Ghostty window {wanted} found. Start Bus (./run dev) or "
        "pass --window ID or --title TEXT."
    )


def capture(window: Window, path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    result = subprocess.run(
        ["screencapture", "-x", "-o", "-l", str(window.id), str(path)],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0 or not path.is_file() or path.stat().st_size == 0:
        raise ScreenshotError(
            f"screencapture failed for window {window.id}: {result.stderr.strip()} "
            f"If {host_app()} lacks Screen Recording permission, allow it in System "
            "Settings > Privacy & Security > Screen Recording."
        )


def screenshot_path(out_dir: Path, now: dt.datetime) -> Path:
    return out_dir / f"bus-{now:%Y%m%d-%H%M%S}.png"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--window", type=int, help="capture this CGWindow id")
    parser.add_argument("--title", help="match windows whose title contains this text")
    parser.add_argument(
        "--out-dir",
        type=Path,
        default=REPO / "temp" / "screenshots",
        help="directory for the PNG (default: temp/screenshots in the repo)",
    )
    args = parser.parse_args(argv)
    try:
        windows = list_windows()
        check_permission(windows)
        window = choose_window(windows, args.window, args.title)
        path = screenshot_path(args.out_dir, dt.datetime.now()).resolve()
        capture(window, path)
    except ScreenshotError as error:
        print(f"bus-screenshot: {error}", file=sys.stderr)
        return 1
    print(f"Captured {window.owner} window {window.id} ({window.title})")
    # The path is the last stdout line, for agents to read it back.
    print(path)
    return 0


if __name__ == "__main__":
    sys.exit(main())
